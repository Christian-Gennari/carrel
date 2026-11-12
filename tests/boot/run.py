#!/usr/bin/env python3
"""Boot and install the Stylos image in QEMU.

Scenarios (all run by `make test-boot`):

  uefi       boot the base image, reach the real editor, screenshot
  bios       same through SeaBIOS + GRUB
  install    boot the base image as a USB stick, install to a blank disk,
             boot the installed disk and screenshot the editor
  recovery   boot the recovery UKI and screenshot its menu
  powercut   type with QMP sendkey, system_reset, boot again and check that
             the text reached /data/writing
  update     stage the offline update fixture, apply it with sysupdate, make
             the new slot fail to reach boot-complete three times and assert
             that the previous slot boots

Only the Python standard library and the tools in dev/Dockerfile are used.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

OVMF_CODE = "/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS = "/usr/share/OVMF/OVMF_VARS_4M.fd"

ESP_GUID = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
BIOS_BOOT_GUID = "21686148-6449-6e6f-744e-656564454649"
ROOT_GUID = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709"
LINUX_GUID = "0fc63daf-8483-4772-8e79-3d69d8477de4"

ANSI_RE = re.compile(r"\x1b\[[0-9;?!]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Za-z0-9]")

KEY = {
    " ": "spc",
    "\n": "ret",
    ".": "dot",
    ",": "comma",
    "-": "minus",
    "/": "slash",
    ":": "shift-semicolon",
    "'": "apostrophe",
}


class BootError(RuntimeError):
    pass


def clean_serial(text: str) -> str:
    return ANSI_RE.sub("", text)


def run(*args: object, **kwargs: object) -> subprocess.CompletedProcess:
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


# --------------------------------------------------------------------------
# GPT and filesystem helpers


def read_gpt(image: Path) -> list[dict]:
    with image.open("rb") as f:
        f.seek(512)
        header = f.read(92)
        if header[:8] != b"EFI PART":
            raise BootError(f"{image} has no GPT header")
        entry_lba, count, entry_size = struct.unpack_from("<QII", header, 72)
        f.seek(entry_lba * 512)
        entries = []
        for _ in range(count):
            entry = f.read(entry_size)
            if len(entry) < 128 or entry[:16] == b"\0" * 16:
                continue
            first, last = struct.unpack_from("<QQ", entry, 32)
            entries.append(
                {
                    "type": str(uuid.UUID(bytes_le=entry[:16])),
                    "first": first,
                    "last": last,
                    "size": (last - first + 1) * 512,
                    "name": entry[56:128].decode("utf-16le").rstrip("\0"),
                }
            )
    return entries


def part_named(parts: list[dict], prefix: str) -> dict:
    for part in parts:
        if part["name"].startswith(prefix):
            return part
    raise BootError(f"no partition named {prefix}*")


def part_typed(parts: list[dict], guid: str) -> dict:
    for part in parts:
        if part["type"] == guid:
            return part
    raise BootError(f"no partition of type {guid}")


@contextlib.contextmanager
def loop_device(image: Path, part: dict):
    dev = subprocess.check_output(
        [
            "losetup", "-f", "--show",
            "--offset", str(part["first"] * 512),
            "--sizelimit", str(part["size"]),
            str(image),
        ],
        text=True,
    ).strip()
    try:
        yield dev
    finally:
        subprocess.run(["losetup", "-d", dev], check=False)


@contextlib.contextmanager
def mounted(image: Path, part: dict, fstype: str, *, writable: bool = False,
            root: Path | None = None):
    if root is not None:
        root.mkdir(parents=True, exist_ok=True)
    with loop_device(image, part) as dev:
        mountpoint = Path(tempfile.mkdtemp(prefix="mnt-", dir=root))
        options = "rw" if writable else "ro"
        try:
            run("mount", "-t", fstype, "-o", options, dev, mountpoint)
        except subprocess.CalledProcessError:
            mountpoint.rmdir()
            raise
        try:
            yield mountpoint
        finally:
            subprocess.run(["umount", mountpoint], check=False)
            subprocess.run(["rmdir", str(mountpoint)], check=False)


def extract_erofs(image: Path, part: dict, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    with loop_device(image, part) as dev:
        run(
            "fsck.erofs", f"--extract={destination}", "--overwrite",
            "--preserve", dev,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )


# --------------------------------------------------------------------------
# QMP


class QMP:
    def __init__(self, path: Path, timeout: float = 30.0) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(10.0)
        deadline = time.monotonic() + timeout
        while True:
            try:
                self.sock.connect(str(path))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if time.monotonic() > deadline:
                    raise BootError(f"timed out waiting for QMP socket {path}")
                time.sleep(0.1)
        self.reader = self.sock.makefile("rb")
        greeting = self._read_message()
        if "QMP" not in greeting:
            raise BootError(f"unexpected QMP greeting: {greeting}")
        self.command("qmp_capabilities")

    def _read_message(self) -> dict:
        while True:
            line = self.reader.readline()
            if not line:
                raise BootError("QMP connection closed")
            msg = json.loads(line)
            if "event" in msg:
                continue
            return msg

    def command(self, name: str, **arguments: object) -> dict:
        payload: dict = {"execute": name}
        if arguments:
            payload["arguments"] = arguments
        self.sock.sendall((json.dumps(payload) + "\n").encode())
        while True:
            msg = self._read_message()
            if "return" in msg:
                return msg["return"]
            if "error" in msg:
                raise BootError(f"{name}: {msg['error'].get('desc', msg['error'])}")

    def close(self) -> None:
        for closeable in (self.reader, self.sock):
            try:
                closeable.close()
            except Exception:
                pass


# --------------------------------------------------------------------------
# One QEMU process


class Qemu:
    def __init__(self, name: str, out: Path, mode: str,
                 disks: list[dict], *, usb: dict | None = None) -> None:
        self.name = name
        self.out = out
        self.mode = mode
        self.disks = disks
        self.usb = usb
        self.dir = out / "test" / name
        self.dir.mkdir(parents=True, exist_ok=True)
        self.serial = self.dir / "serial.log"
        self.qmp_path = self.dir / "qmp.sock"
        self.log = self.dir / "qemu.log"
        self.vars = self.dir / "OVMF_VARS.fd"
        self.proc: subprocess.Popen | None = None
        self.qmp: QMP | None = None
        self.pos = 0

    def _command(self) -> list[str]:
        cmd = [
            "qemu-system-x86_64",
            "-name", f"stylos-{self.name}",
            "-machine", "q35,accel=kvm:tcg",
            "-m", "2048",
            "-smp", "2",
            "-display", "none",
            "-vga", "std",
            "-boot", "order=c,menu=off",
            "-nic", "none",
            "-device", "virtio-rng-pci",
            "-serial", f"file:{self.serial}",
            "-monitor", "none",
            "-qmp", f"unix:{self.qmp_path},server=on,wait=off",
        ]

        if self.mode == "uefi":
            if not Path(OVMF_CODE).exists() or not Path(OVMF_VARS).exists():
                raise BootError("OVMF firmware is not installed in the dev container")
            if not self.vars.exists():
                shutil.copyfile(OVMF_VARS, self.vars)
            cmd[cmd.index("-boot"):cmd.index("-boot") + 2] = []
            cmd += [
                "-drive", f"if=pflash,format=raw,readonly=on,file={OVMF_CODE}",
                "-drive", f"if=pflash,format=raw,file={self.vars}",
            ]

        if self.usb is not None:
            cmd += ["-device", "qemu-xhci,id=xhci"]
            cmd += [
                "-drive", f"if=none,id=usb,format={self.usb['format']},file={self.usb['path']}",
                "-device", "usb-storage,drive=usb,bootindex=0",
            ]

        for index, disk in enumerate(self.disks):
            ident = f"disk{index}"
            cmd += [
                "-drive", f"if=none,id={ident},format={disk['format']},file={disk['path']}",
                "-device", f"virtio-blk-pci,drive={ident},bootindex={index + 1}",
            ]
        return cmd

    def __enter__(self) -> "Qemu":
        self.qmp_path.unlink(missing_ok=True)
        self.serial.unlink(missing_ok=True)
        with self.log.open("w") as log:
            self.proc = subprocess.Popen(
                self._command(), stdout=log, stderr=subprocess.STDOUT,
            )
        try:
            self.qmp = QMP(self.qmp_path, timeout=30)
        except BootError:
            self.stop()
            raise
        return self

    def __exit__(self, *exc: object) -> None:
        self.stop()

    def text(self) -> str:
        try:
            return clean_serial(self.serial.read_text(errors="replace"))
        except FileNotFoundError:
            return ""

    def wait(self, pattern: str, timeout: float = 240.0) -> str:
        """Wait for a regex after the last consumed position."""
        deadline = time.monotonic() + timeout
        while True:
            text = self.text()
            match = re.search(pattern, text[self.pos:])
            if match:
                self.pos += match.end()
                return text
            if time.monotonic() > deadline:
                raise BootError(
                    f"[{self.name}] timed out waiting for {pattern!r}\n"
                    f"--- serial tail ---\n{self.serial_tail()}"
                )
            time.sleep(0.4)

    def wait_fresh(self, pattern: str, timeout: float = 240.0) -> str:
        self.pos = len(self.text())
        return self.wait(pattern, timeout)

    def serial_tail(self, lines: int = 120) -> str:
        return "\n".join(self.text().splitlines()[-lines:])

    def key(self, keys: str, hold_ms: int = 60) -> None:
        assert self.qmp is not None
        # QMP's command is send-key (sendkey is the HMP spelling).  A chord
        # such as shift-e is an array of qcodes pressed together.
        qcodes = [{"type": "qcode", "data": part} for part in keys.split("-")]
        self.qmp.command("send-key", keys=qcodes, **{"hold-time": hold_ms})

    def type_text(self, text: str, delay: float = 0.06) -> None:
        for char in text:
            if char.isupper():
                keys = f"shift-{char.lower()}"
            elif char in KEY:
                keys = KEY[char]
            else:
                keys = char
            self.key(keys)
            time.sleep(delay)

    def screenshot(self, path: Path, *, light: bool = True) -> Path:
        assert self.qmp is not None
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            self.qmp.command("screendump", filename=str(path), format="png")
        except BootError:
            ppm = path.with_suffix(".ppm")
            self.qmp.command("screendump", filename=str(ppm))
            run("convert", ppm, path)
            ppm.unlink(missing_ok=True)
        if not path.exists() or path.stat().st_size < 2048:
            raise BootError(f"[{self.name}] screenshot looks empty: {path}")
        if shutil.which("identify"):
            values = subprocess.run(
                ["identify", "-format", "%[fx:mean] %[fx:standard_deviation]", str(path)],
                check=True, capture_output=True, text=True,
            ).stdout.split()
            mean, stddev = float(values[0]), float(values[1])
            if light and mean < 0.5:
                raise BootError(
                    f"[{self.name}] screenshot too dark (mean={mean}); editor missing?"
                )
            if light and stddev < 0.01:
                raise BootError(
                    f"[{self.name}] screenshot has no contrast (stddev={stddev})"
                )
        return path

    def reset(self) -> None:
        assert self.qmp is not None
        self.qmp.command("system_reset")

    def powerdown(self, timeout: float = 90.0) -> None:
        assert self.qmp is not None
        try:
            self.qmp.command("system_powerdown")
        except BootError:
            pass
        deadline = time.monotonic() + timeout
        while self.proc is not None and self.proc.poll() is None:
            if time.monotonic() > deadline:
                self.stop()
                return
            time.sleep(0.5)
        self.qmp is not None and self.qmp.close()
        self.qmp = None

    def stop(self) -> None:
        if self.qmp is not None:
            try:
                self.qmp.command("quit")
            except Exception:
                pass
            self.qmp.close()
            self.qmp = None
        if self.proc is not None:
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
            self.proc = None
        self.qmp_path.unlink(missing_ok=True)


# --------------------------------------------------------------------------
# Offline image checks


def expect(condition: bool, message: str, failures: list[str]) -> None:
    if not condition:
        failures.append(message)


def check_image(image: Path, update_dir: Path, out: Path) -> None:
    parts = read_gpt(image)
    failures: list[str] = []

    esp = part_typed(parts, ESP_GUID)
    bios = part_typed(parts, BIOS_BOOT_GUID)
    roots = sorted(
        (p for p in parts if p["name"].startswith("stylos-root-")),
        key=lambda p: p["first"],
    )
    if len(roots) != 2:
        raise BootError(f"expected two root slots, found {len(roots)}")
    root_a, root_b = roots
    data = part_named(parts, "stylos-data")

    expect(esp["type"] == ESP_GUID, "ESP type is wrong", failures)
    expect(bios["size"] >= 1024 * 1024, "BIOS boot partition is too small", failures)
    expect(root_a["size"] == 2 * 1024**3, "root-a is not 2 GiB", failures)
    expect(root_b["size"] == 2 * 1024**3, "root-b is not 2 GiB", failures)
    expect(data["type"] == LINUX_GUID, "data is not linux-generic", failures)
    expect(data["size"] >= 2 * 1024**3, "data partition is smaller than 2 GiB", failures)
    expect(esp["first"] < bios["first"] < root_a["first"] < root_b["first"] < data["first"],
           "partition order is not ESP, BIOS, root-a, root-b, data", failures)

    # ESP: the bootloader, both slot UKIs and the recovery entry.
    with mounted(image, esp, "vfat", root=out / "test") as boot:
        for name in ("stylos-a.efi", "stylos-0-recovery.efi"):
            path = boot / "EFI/Linux" / name
            expect(path.is_file() and path.stat().st_size > 50 * 1024**2,
                   f"ESP is missing a full {name}", failures)
        loader = (boot / "loader/loader.conf").read_text()
        expect("default stylos-a.efi" in loader, "loader.conf does not default to slot A",
               failures)
        grub = (boot / "grub/grub.cfg").read_text()
        expect("Stylos recovery" in grub, "GRUB menu has no recovery entry", failures)
        expect("save_env stylos_b_tries" in grub,
               "GRUB menu has no boot counter", failures)
        expect(any((boot / "stylos").glob("*/vmlinuz")),
               "ESP has no kernel for the BIOS path", failures)

    # Root-A: the real product.
    extracted = out / "test" / "image-check" / "root-a"
    if extracted.exists():
        shutil.rmtree(extracted)
    extracted.parent.mkdir(parents=True, exist_ok=True)
    extract_erofs(image, root_a, extracted)

    def root_file(path: str) -> Path:
        return extracted / path.lstrip("/")

    for binary, minimum in (
        ("/usr/bin/stylos-shell", 500 * 1024),
        ("/usr/bin/stylosd", 5 * 1024**2),
        ("/usr/bin/pandoc", 1024),
        ("/usr/bin/typst", 1024),
        ("/usr/bin/cage", 1024),
    ):
        path = root_file(binary)
        size = path.stat().st_size if path.exists() else 0
        expect(size >= minimum, f"{binary} is missing or too small ({size} bytes)", failures)
        if path.exists():
            expect(path.read_bytes()[:4] == b"\x7fELF", f"{binary} is not an ELF", failures)

    # The Rust shell links GtkSourceView 5, not the placeholder.
    expect(b"libgtksourceview-5" in root_file("/usr/bin/stylos-shell").read_bytes(),
           "stylos-shell does not link GtkSourceView 5", failures)

    for path in (
        "/usr/lib/systemd/system/stylosd.service",
        "/usr/share/dbus-1/system.d/org.stylos.Daemon1.conf",
        "/usr/lib/stylos/installer",
        "/usr/lib/stylos/recovery",
        "/usr/lib/stylos/apply-update",
        "/usr/lib/stylos/boot-complete",
        "/usr/lib/stylos/data-init",
        "/usr/lib/stylos/grow-data",
        "/usr/lib/stylos/sysupdate/root.transfer",
        "/usr/lib/stylos/sysupdate/uki-a.transfer",
        "/usr/lib/stylos/sysupdate/uki-b.transfer",
        "/usr/lib/stylos/repart-grow/50-data.conf",
    ):
        expect(root_file(path).exists(), f"missing {path}", failures)

    enabled = root_file("/etc/systemd/system/multi-user.target.wants/stylosd.service")
    expect(enabled.is_symlink() or enabled.exists(), "stylosd.service is not enabled", failures)

    session = root_file("/etc/systemd/system/stylos-session.service").read_text()
    exec_line = next(
        (line.strip() for line in session.splitlines() if line.startswith("ExecStart=")),
        "",
    )
    expect(
        re.fullmatch(
            r"ExecStart=/usr/bin/cage(?: -[a-z]+)* -- /usr/bin/stylos-shell",
            exec_line,
        ) is not None,
        f"session does not start cage with stylos-shell: {exec_line!r}",
        failures,
    )
    expect(" -s" not in exec_line, "session passes cage -s (VT switching)", failures)
    expect("Restart=always" in session, "session does not restart", failures)
    expect("User=writer" in session, "session does not run as writer", failures)

    for unit in (
        "getty@.service", "serial-getty@.service", "console-getty.service",
        "autovt@.service", "systemd-networkd.service", "systemd-timesyncd.service",
    ):
        link = root_file(f"/etc/systemd/system/{unit}")
        expect(link.is_symlink() and str(link.readlink()) == "/dev/null",
               f"{unit} is not masked", failures)

    for binary in (
        "/usr/bin/foot", "/usr/bin/xterm", "/usr/bin/alacritty", "/usr/bin/kitty",
        "/usr/bin/gnome-terminal", "/usr/bin/konsole", "/usr/bin/weston-terminal",
    ):
        expect(not root_file(binary).exists(), f"terminal emulator installed: {binary}",
               failures)

    fonts = root_file("/usr/share/fonts")
    names = " ".join(p.name.lower() for p in fonts.rglob("*") if p.is_file()) \
        if fonts.exists() else ""
    expect("sourceserif" in names, "Source Serif 4 is not installed", failures)
    expect("inter" in names, "Inter is not installed", failures)
    expect("jetbrains" in names, "JetBrains Mono is not installed", failures)

    fstab = root_file("/etc/fstab").read_text()
    expect("PARTUUID=33333333-3333-4333-8333-333333333333 /data" in fstab,
           "fstab does not mount data", failures)
    expect("PARTUUID=44444444-4444-4444-8444-444444444444 /boot" in fstab,
           "fstab does not mount the ESP", failures)

    passwd = root_file("/etc/passwd").read_text()
    expect("writer:x:1000:1000:" in passwd, "writer user is missing", failures)
    writing = root_file("/data/writing")
    expect(writing.is_dir(), "/data/writing is missing", failures)

    oauth = root_file("/usr/share/stylos/oauth.toml")
    if oauth.exists():
        print("image: oauth.toml present", flush=True)
    else:
        print("image: no oauth.toml (built without Google Drive client IDs)", flush=True)

    # Root-B is the same image.  Check the erofs integrity rather than
    # extracting another 1.5 GiB.
    with loop_device(image, root_b) as dev:
        result = subprocess.run(["fsck.erofs", "-d0", dev],
                                capture_output=True, text=True)
        expect(result.returncode == 0, "root-b is not a valid erofs image", failures)

    for name in (
        "payload/stylos-root-0.2.0.raw",
        "payload/stylos-b-0.2.0.efi",
        "README",
    ):
        expect((update_dir / name).exists(), f"missing update fixture {name}", failures)

    shutil.rmtree(extracted, ignore_errors=True)
    if failures:
        raise BootError("; ".join(failures))
    print("image: offline checks passed", flush=True)


# --------------------------------------------------------------------------
# Runtime checks


def runtime_check(qemu: Qemu) -> None:
    text = qemu.text()
    failures = []
    if qemu.mode == "uefi":
        expect("EFI stub" in text, "UEFI boot did not use the EFI stub", failures)
    else:
        expect("BOOT_IMAGE=" in text, "BIOS boot did not use GRUB", failures)
        expect("EFI stub" not in text, "BIOS boot unexpectedly used the EFI stub", failures)

    expect(not re.search(r"getty@tty[1-6]\.service", text),
           "a getty started on tty1-6", failures)
    expect("serial-getty@" not in text, "a serial getty started", failures)
    expect("Started systemd-networkd.service" not in text,
           "systemd-networkd started", failures)
    expect("Started systemd-timesyncd.service" not in text,
           "systemd-timesyncd started", failures)
    expect("Kernel panic" not in text, "kernel panic", failures)
    expect(text.count("Started stylos-session.service") == 1,
           "stylos-session started more than once", failures)
    expect("Reached target graphical.target" in text,
           "graphical.target was not reached", failures)
    expect("Started stylosd.service" in text, "stylosd did not start", failures)
    expect("Finished stylos-rfkill.service" in text, "stylos-rfkill did not run", failures)
    expect("Finished stylos-grow-data.service" in text,
           "stylos-grow-data did not finish", failures)
    expect("STYLOS-DATA: /var is on data" in text, "/var is not on data", failures)
    expect("STYLOS-DATA: /etc overlay is on data" in text,
           "/etc overlay is not on data", failures)
    expect(re.search(r"STYLOS-BOOT: slot [ab] marked good", text) is not None,
           "the running slot was not marked good (boot-complete failed)", failures)
    if failures:
        raise BootError("; ".join(failures))


def boot_to_editor(qemu: Qemu, timeout: float) -> None:
    qemu.wait(r"Started stylos-session\.service", timeout)
    # Mark-good is pulled in by graphical.target and ordered after the session,
    # so it logs before the target itself is reached.
    qemu.wait(r"STYLOS-BOOT: slot [ab] marked good", timeout)
    qemu.wait(r"Reached target graphical\.target", timeout)
    time.sleep(5)  # let GTK draw the first frame


# --------------------------------------------------------------------------
# Scenarios


def qcow2_overlay(base: Path, path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    run("qemu-img", "create", "-q", "-f", "qcow2", "-F", "raw",
        "-b", base, path)
    return path


def scenario_boot(mode: str, image: Path, out: Path, timeout: float) -> None:
    overlay = qcow2_overlay(image, out / "test" / f"boot-{mode}" / "disk.qcow2")
    with Qemu(f"boot-{mode}", out, mode,
              [{"path": overlay, "format": "qcow2"}]) as qemu:
        boot_to_editor(qemu, timeout)
        runtime_check(qemu)
        # Type one line so the screenshot shows the real editor (and proves
        # that QMP keyboard input reaches stylos-shell).
        qemu.type_text("The quiet morning")
        time.sleep(1.5)
        qemu.screenshot(out / f"screenshot-{mode}.png")
        print(f"{mode}: PASS", flush=True)


def wait_installer(qemu: Qemu, marker: str, timeout: float = 600.0) -> None:
    qemu.wait(rf"STYLOS-INSTALLER: {re.escape(marker)}", timeout)


def scenario_install(image: Path, out: Path, timeout: float) -> Path:
    base = out / "test" / "install"
    base.mkdir(parents=True, exist_ok=True)
    source = qcow2_overlay(image, base / "usb.qcow2")
    target = base / "disk.img"
    if target.exists():
        target.unlink()
    run("qemu-img", "create", "-q", "-f", "raw", target, "16G")

    with Qemu("install", out, "uefi", [{"path": target, "format": "raw"}],
              usb={"path": source, "format": "qcow2"}) as qemu:
        wait_installer(qemu, "main menu", timeout)
        qemu.screenshot(out / "screenshot-install-menu.png", light=False)
        qemu.key("down")
        qemu.key("ret")
        wait_installer(qemu, "disk menu", 120)
        qemu.key("ret")
        wait_installer(qemu, "erase prompt", 120)
        qemu.type_text("ERASE")
        qemu.key("ret")
        wait_installer(qemu, "installing", 120)
        wait_installer(qemu, "complete", timeout)
        qemu.stop()

    parts = read_gpt(target)
    data = part_named(parts, "stylos-data")
    if data["size"] < 10 * 1024**3:
        raise BootError(f"installer did not grow data: {data['size'] // 1024**3} GiB")

    # Boot the installed disk without the USB stick.
    with Qemu("installed", out, "uefi", [{"path": target, "format": "raw"}]) as qemu:
        boot_to_editor(qemu, timeout)
        runtime_check(qemu)
        qemu.type_text("The quiet morning")
        time.sleep(1.5)
        qemu.screenshot(out / "screenshot-install.png")
        qemu.powerdown()
    print("install: PASS", flush=True)
    return target


def scenario_recovery(disk: Path, out: Path, timeout: float) -> None:
    parts = read_gpt(disk)
    esp = part_typed(parts, ESP_GUID)
    loader_path = "loader/loader.conf"
    with mounted(disk, esp, "vfat", writable=True, root=out / "test") as boot:
        loader = boot / loader_path
        original = loader.read_text()
        loader.write_text("default stylos-0-recovery.efi\ntimeout 2\n")
    try:
        with Qemu("recovery", out, "uefi", [{"path": disk, "format": "raw"}]) as qemu:
            qemu.wait(r"STYLOS-RECOVERY: menu", timeout)
            time.sleep(3)  # whiptail draws over the last kernel console lines
            qemu.screenshot(out / "screenshot-recovery.png", light=False)
            if "Started stylos-session.service" in qemu.text():
                raise BootError("the session started while booted into recovery")
            qemu.stop()
    finally:
        with mounted(disk, esp, "vfat", writable=True, root=out / "test") as boot:
            (boot / loader_path).write_text(original)
    print("recovery: PASS", flush=True)


def scenario_powercut(disk: Path, out: Path, timeout: float) -> None:
    with Qemu("powercut", out, "uefi", [{"path": disk, "format": "raw"}]) as qemu:
        boot_to_editor(qemu, timeout)
        qemu.type_text("hello power cut")
        time.sleep(2.5)  # journal fdatasync is at most 1 s behind
        qemu.reset()
        qemu.wait_fresh(r"stylos\.slot=a", timeout)
        qemu.wait(r"Started stylos-session\.service", timeout)
        time.sleep(8)  # replay the journal and autosave after 5 s idle
        qemu.screenshot(out / "screenshot-powercut-recovered.png")
        qemu.powerdown()

    parts = read_gpt(disk)
    data = part_named(parts, "stylos-data")
    found = []
    with mounted(disk, data, "ext4", root=out / "test") as root:
        for path in (root / "writing").rglob("*"):
            if path.is_file():
                found.append(path.read_text(errors="replace"))
    if not any("hello power cut" in text for text in found):
        raise BootError("text did not survive the power cut in /data/writing")
    print("powercut: PASS", flush=True)


def stage_update(disk: Path, update_dir: Path, out: Path) -> None:
    parts = read_gpt(disk)
    data = part_named(parts, "stylos-data")
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        payload = root / ".stylos" / "update" / "payload"
        payload.mkdir(parents=True, exist_ok=True)
        for source in sorted((update_dir / "payload").iterdir()):
            destination = payload / source.name
            if destination.exists():
                destination.unlink()
            run("cp", "--sparse=always", source, destination)


def scenario_update(disk: Path, update_dir: Path, out: Path, timeout: float) -> None:
    stage_update(disk, update_dir, out)

    with Qemu("update", out, "uefi", [{"path": disk, "format": "raw"}]) as qemu:
        qemu.wait(r"STYLOS-UPDATE: applying", timeout)
        qemu.wait(r"STYLOS-UPDATE: .*applied", timeout)

        # The update rebooted into slot B.  Slot B masks the session and has
        # a failing unit, so boot-complete is never reached.  After the third
        # boot the OS removes the exhausted entry and reboots into A.
        for attempt in range(1, 4):
            qemu.wait(r"stylos\.slot=b", timeout)
            print(f"update: slot B attempt {attempt}", flush=True)
            time.sleep(12)
            if attempt < 3:
                qemu.reset()

        qemu.wait(r"stylos\.slot=a", timeout)
        qemu.wait(r"Started stylos-session\.service", timeout)
        qemu.wait(r"STYLOS-BOOT: slot a marked good", timeout)
        time.sleep(5)
        qemu.screenshot(out / "screenshot-update-rollback.png")
        qemu.powerdown()
    print("update: PASS", flush=True)


# --------------------------------------------------------------------------
# Entry point


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--update-dir", type=Path,
                        help="directory with the offline update fixture")
    parser.add_argument("--scenario", default="all",
                        choices=("all", "checks", "uefi", "bios", "install",
                                 "recovery", "powercut", "update"))
    parser.add_argument("--timeout", type=float, default=300.0,
                        help="seconds for one boot (default 300)")
    args = parser.parse_args()

    args.image = args.image.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    if not args.image.exists():
        print(f"image not found: {args.image}", file=sys.stderr)
        return 2
    if args.update_dir is None:
        args.update_dir = args.image.parent / "update"
    args.update_dir = args.update_dir.resolve()

    if args.scenario in ("all", "checks"):
        try:
            check_image(args.image, args.update_dir, args.out)
        except BootError as exc:
            print(f"FAILED: {exc}", file=sys.stderr)
            return 1
        if args.scenario == "checks":
            return 0

    errors: list[str] = []
    installed: Path | None = None
    try:
        if args.scenario in ("all", "uefi"):
            scenario_boot("uefi", args.image, args.out, args.timeout)
        if args.scenario in ("all", "bios"):
            scenario_boot("bios", args.image, args.out, args.timeout)
        if args.scenario in ("all", "install", "recovery", "powercut", "update"):
            reused = args.out / "test" / "install" / "disk.img"
            if args.scenario != "all" and reused.exists():
                installed = reused
                print(f"reusing installed disk {installed}", flush=True)
            else:
                installed = scenario_install(args.image, args.out, args.timeout)
        if args.scenario in ("all", "recovery"):
            scenario_recovery(installed, args.out, args.timeout)
        if args.scenario in ("all", "powercut"):
            scenario_powercut(installed, args.out, args.timeout)
        if args.scenario in ("all", "update"):
            scenario_update(installed, args.update_dir, args.out, args.timeout)
    except BootError as exc:
        errors.append(str(exc))
        print(f"FAILED: {exc}", file=sys.stderr)

    if errors:
        return 1
    print(f"all scenarios passed; screenshots in {args.out}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
