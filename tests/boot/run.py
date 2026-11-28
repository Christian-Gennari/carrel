#!/usr/bin/env python3
"""Boot and install the Carrel image in QEMU.

Scenarios (all run by `make test-boot`):

  uefi       boot the base image, reach the real editor, screenshot
  bios       same through SeaBIOS + GRUB
  install    boot the base image as a USB stick, install to a blank disk,
             boot the installed disk and screenshot the editor
  recovery   boot the recovery entry, copy the writing to ext4, FAT and exFAT
             USB drives, check the copies, then reset settings and power off
  powercut   type with QMP sendkey, system_reset, boot again and check that
             the text reached /data/writing
  shutdown-sync  turn "Sync at shutdown" on, pair a server that never answers,
             type, press the power button and check that the text was saved
             before the sync, that the sync was tried and abandoned at its time
             limit (journal), and that power off finished
  palette-power  run Suspend, Restart and Shut down from the Command palette:
             the VM sleeps and wakes, comes back up, then powers off; the text
             typed just before each one is saved, and Sync at shutdown runs
  typing-slow-disk  start Carrel from a USB drive that writes like a slow stick,
             type across autosaves, a new file's naming and two sync rounds
             against a local server, and fail on any main-loop stall over
             50 ms in the shell's stall log
  update     stage the offline update fixture, apply it with sysupdate, make
             the new slot fail to reach boot-complete three times and assert
             that the previous slot boots
  secureboot boot the image, install it to a blank disk and boot the installed
             disk, all in QEMU with OVMF's Microsoft keys enrolled and Secure
             Boot enforcing (shim -> Debian systemd-boot -> Debian kernel);
             assert the installer made Carrel the first UEFI boot entry
  update-secureboot  update + forced rollback on the Secure Boot install
  release    flash the release .img.xz onto a fresh, larger virtual external
             drive and choose Start Carrel on UEFI, UEFI Secure Boot and BIOS
  actions    walk the user-facing action inventory (spec/actions.md) via QMP
             keys, checking visible OCR effects and /data files across a reboot
             (pass --swedish to also check å ä ö typing)

Only the Python standard library and the tools in dev/Dockerfile are used.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import http.server
import json
import re
import shutil
import socket
import ssl
import struct
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pefile

OVMF_CODE = "/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS = "/usr/share/OVMF/OVMF_VARS_4M.fd"
# Secure Boot enforcing, with the Microsoft keys (and so Debian's shim)
# enrolled.  These need SMM, see Qemu._command.
OVMF_CODE_MS = "/usr/share/OVMF/OVMF_CODE_4M.ms.fd"
OVMF_VARS_MS = "/usr/share/OVMF/OVMF_VARS_4M.ms.fd"

ESP_GUID = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
BIOS_BOOT_GUID = "21686148-6449-6e6f-744e-656564454649"
ROOT_GUID = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709"
LINUX_GUID = "0fc63daf-8483-4772-8e79-3d69d8477de4"
VERITY_GUID = "2c7357ed-ebd2-46d9-aec1-23d437ec2bf5"
VERITY_SIG_GUID = "41092b05-9fc8-4523-994f-2def0408b176"
ROOT_A_UUID = "11111111-1111-4111-8111-111111111111"
ROOT_B_UUID = "22222222-2222-4222-8222-222222222222"
HASH_A_UUID = "55555555-5555-4555-8555-555555555555"
HASH_B_UUID = "66666666-6666-4666-8666-666666666666"
SIG_A_UUID = "77777777-7777-4777-8777-777777777777"
SIG_B_UUID = "88888888-8888-4888-8888-888888888888"


def tool(name: str) -> str:
    """mkfs/sfdisk live in /usr/sbin, which is not always on PATH."""
    return shutil.which(name) or f"/usr/sbin/{name}"


def grubenv_get(path: Path, key: str) -> str:
    listing = subprocess.run(
        ["grub-editenv", path, "list"],
        check=True, capture_output=True, text=True,
    ).stdout
    for line in listing.splitlines():
        name, _, value = line.partition("=")
        if name == key:
            return value
    return ""

ANSI_RE = re.compile(r"\x1b\[[0-9;?!]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Za-z0-9]")
# 0.1.0 has no audit=0 on the kernel command line, so audit lines can land in
# the middle of a systemd line and even inside an ANSI escape sequence on the
# serial console.  Removing them (and their newline) lets the split line join
# again before ANSI_RE runs.
AUDIT_RE = re.compile(
    r"\[\s*\d+\.\d+\][^\r\n]*(?:audit:|kauditd_printk_skb)[^\r\n]*(?:\r?\n)?"
)

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
    return ANSI_RE.sub("", AUDIT_RE.sub("", text))


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
                    "uuid": str(uuid.UUID(bytes_le=entry[16:32])),
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


def part_by_uuid(parts: list[dict], uuid: str) -> dict:
    for part in parts:
        if part["uuid"] == uuid:
            return part
    raise BootError(f"no partition with UUID {uuid}")


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
            root: Path | None = None, extra_options: str = ""):
    if root is not None:
        root.mkdir(parents=True, exist_ok=True)
    with loop_device(image, part) as dev:
        mountpoint = Path(tempfile.mkdtemp(prefix="mnt-", dir=root))
        options = "rw" if writable else "ro"
        if extra_options:
            options += f",{extra_options}"
        try:
            run("mount", "-t", fstype, "-o", options, dev, mountpoint)
        except subprocess.CalledProcessError:
            mountpoint.rmdir()
            raise
        try:
            yield mountpoint
        finally:
            result = subprocess.run(["umount", mountpoint], check=False)
            if result.returncode != 0:
                # A library may still hold a mapping; detach it so the next
                # loop device and mount are not poisoned.
                subprocess.run(["umount", "-l", mountpoint], check=False)
            subprocess.run(["rmdir", str(mountpoint)], check=False)


def extract_erofs(image: Path, part: dict, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    with loop_device(image, part) as dev:
        run(
            "fsck.erofs", f"--extract={destination}", "--overwrite",
            "--preserve", dev,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )


def entry_options(entry: Path) -> str:
    """The kernel command line of a Boot Loader Specification Type #1 entry."""
    for line in entry.read_text().splitlines():
        if line.startswith("options"):
            return line.partition(" ")[2].strip()
    return ""


def entry_value(entry: Path, key: str) -> str:
    for line in entry.read_text().splitlines():
        name, _, value = line.partition(" ")
        if name == key:
            return value.strip()
    return ""


def pe_is_signed(binary: Path) -> bool:
    pe = pefile.PE(str(binary), fast_load=True)
    try:
        # IMAGE_DIRECTORY_ENTRY_SECURITY (index 4) holds the Authenticode blob.
        return pe.OPTIONAL_HEADER.DATA_DIRECTORY[4].VirtualAddress != 0
    finally:
        pe.close()


def ocr(path: Path) -> str:
    result = subprocess.run(
        ["tesseract", str(path), "stdout"],
        check=True, capture_output=True, text=True,
    )
    return result.stdout


@contextlib.contextmanager
def serve_directory(directory: Path):
    """Serve a release directory on a free localhost port for QEMU's user net."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    proc = subprocess.Popen(
        [sys.executable, "-m", "http.server", str(port), "--bind", "0.0.0.0"],
        cwd=directory,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                    break
            except OSError:
                time.sleep(0.1)
        else:
            raise BootError(f"HTTP server for {directory} did not start")
        yield port
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


# Filesystem: (MBR partition type, mkfs command, options to read it back).
USB_FILESYSTEMS = {
    "ext4": ("83", ("mkfs.ext4", "-q", "-F"), ""),
    "vfat": ("0c", ("mkfs.vfat",), "utf8"),
    "exfat": ("07", ("mkfs.exfat",), ""),
}


def make_usb_disk(path: Path, fstype: str, size: str = "512M") -> dict:
    """A small raw disk with one partition, for the recovery copy test."""
    kind, mkfs, _ = USB_FILESYSTEMS[fstype]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    run("truncate", "-s", size, path)
    run(
        tool("sfdisk"), path,
        input=f"label: dos\nstart=2048, size=1046528, type={kind}\n",
        text=True,
    )
    part = {"first": 2048, "size": 1046528 * 512}
    with loop_device(path, part) as dev:
        run(tool(mkfs[0]), *mkfs[1:], dev, stdout=subprocess.DEVNULL)
    return part


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


def next_noon_utc() -> str:
    """QEMU's -rtc base for the next 12:00 UTC.

    The default theme is Automatic: a night theme after dark. The screenshot
    checks expect the day theme, so every VM wakes up at noon (the device's
    default time zone is UTC). The next noon, not today's, so the clock is
    never behind the release signatures made just before the test.
    """
    now = datetime.now(timezone.utc)
    noon = now.replace(hour=12, minute=0, second=0, microsecond=0)
    if noon <= now:
        noon += timedelta(days=1)
    return noon.strftime("%Y-%m-%dT%H:%M:%S")


class Qemu:
    def __init__(self, name: str, out: Path, mode: str,
                 disks: list[dict], *, usb: dict | None = None,
                 network: bool = False, secure: bool = False,
                 vars_from: Path | None = None) -> None:
        self.name = name
        self.secure = secure
        self.vars_from = vars_from
        self.out = out
        self.mode = mode
        self.disks = disks
        self.usb = usb
        self.network = network
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
            "-name", f"carrel-{self.name}",
            "-machine",
            "q35,accel=kvm:tcg" + (",smm=on" if self.secure else ""),
            "-m", "2048",
            "-smp", "2",
            "-display", "none",
            "-vga", "std",
            "-rtc", f"base={next_noon_utc()}",
            "-boot", "order=c,menu=off",
            "-device", "virtio-rng-pci",
            "-serial", f"file:{self.serial}",
            "-monitor", "none",
            "-qmp", f"unix:{self.qmp_path},server=on,wait=off",
        ]
        if self.network:
            # 10.0.2.2 is the host from inside QEMU's user-mode network.
            cmd += [
                "-netdev", "user,id=net0",
                "-device", "virtio-net-pci,netdev=net0",
            ]
        else:
            cmd += ["-nic", "none"]

        if self.mode == "uefi":
            code = OVMF_CODE_MS if self.secure else OVMF_CODE
            template = OVMF_VARS_MS if self.secure else OVMF_VARS
            if not Path(code).exists() or not Path(template).exists():
                raise BootError("OVMF firmware is not installed in the dev container")
            if not self.vars.exists():
                # Start from the NVRAM of an earlier run (the installer's boot
                # entry lives there) or from a clean template.
                source = self.vars_from if self.vars_from is not None else Path(template)
                shutil.copyfile(source, self.vars)
            cmd[cmd.index("-boot"):cmd.index("-boot") + 2] = []
            if self.secure:
                # SMM-protected variable store: Secure Boot cannot be turned
                # off from inside the guest.
                cmd += [
                    "-global", "driver=cfi.pflash01,property=secure,value=on",
                    "-global", "ICH9-LPC.disable_s3=1",
                ]
            cmd += [
                "-drive", f"if=pflash,format=raw,readonly=on,file={code}",
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
        self.vars.unlink(missing_ok=True)
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
        # On a busy machine the first frame can arrive late (TCG, shared
        # CPU): retry a dark or empty screen for a while before failing.
        problem = ""
        for attempt in range(20):
            problem = self._screendump(path, light)
            if not problem:
                return path
            time.sleep(3)
        raise BootError(problem)

    def _screendump(self, path: Path, light: bool) -> str:
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
            return f"[{self.name}] screenshot looks empty: {path}"
        if shutil.which("identify"):
            values = subprocess.run(
                ["identify", "-format", "%[fx:mean] %[fx:standard_deviation]", str(path)],
                check=True, capture_output=True, text=True,
            ).stdout.split()
            mean, stddev = float(values[0]), float(values[1])
            if light and mean < 0.5:
                return f"[{self.name}] screenshot too dark (mean={mean}); editor missing?"
            if light and stddev < 0.01:
                return f"[{self.name}] screenshot has no contrast (stddev={stddev})"
        return ""

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
        (p for p in parts if p["name"].startswith("carrel-root-")),
        key=lambda p: p["first"],
    )
    if len(roots) != 2:
        raise BootError(f"expected two root slots, found {len(roots)}")
    root_a = part_by_uuid(parts, ROOT_A_UUID)
    root_b = part_by_uuid(parts, ROOT_B_UUID)
    verity_a = part_by_uuid(parts, HASH_A_UUID)
    verity_b = part_by_uuid(parts, HASH_B_UUID)
    sig_a = part_by_uuid(parts, SIG_A_UUID)
    sig_b = part_by_uuid(parts, SIG_B_UUID)
    data = part_named(parts, "carrel-data")

    expect(esp["type"] == ESP_GUID, "ESP type is wrong", failures)
    expect(bios["size"] >= 1024 * 1024, "BIOS boot partition is too small", failures)
    expect(root_a["size"] == 2 * 1024**3, "root-a is not 2 GiB", failures)
    expect(root_b["size"] == 2 * 1024**3, "root-b is not 2 GiB", failures)
    expect(verity_a["type"] == VERITY_GUID, "root-a has no verity hash partition", failures)
    expect(verity_b["type"] == VERITY_GUID, "root-b has no verity hash partition", failures)
    expect(verity_a["size"] == 64 * 1024**2, "root-a verity hash is not 64 MiB", failures)
    expect(verity_b["size"] == 64 * 1024**2, "root-b verity hash is not 64 MiB", failures)
    expect(sig_a["type"] == VERITY_SIG_GUID, "root-a has no verity signature partition", failures)
    expect(sig_b["type"] == VERITY_SIG_GUID, "root-b has no verity signature partition", failures)
    expect(sig_a["size"] >= 4096, "root-a verity signature partition is too small", failures)
    expect(sig_b["size"] >= 4096, "root-b verity signature partition is too small", failures)
    expect(data["type"] == LINUX_GUID, "data is not linux-generic", failures)
    expect(data["size"] >= 2 * 1024**3, "data partition is smaller than 2 GiB", failures)
    expect(
        esp["first"] < bios["first"] < root_a["first"] < verity_a["first"]
        < sig_a["first"] < root_b["first"] < verity_b["first"] < sig_b["first"]
        < data["first"],
        "partition order is not ESP, BIOS, root-a, verity-a, sig-a, root-b, "
        "verity-b, sig-b, data",
        failures,
    )

    # ESP: the Secure Boot chain, the A/B boot entries and the recovery entry.
    with mounted(image, esp, "vfat", root=out / "test") as boot:
        work = out / "test" / "esp-check"
        work.mkdir(parents=True, exist_ok=True)
        # Debian's signed binaries, unchanged.  shim is BOOTX64.EFI on
        # removable media and under EFI/carrel for the firmware boot entry;
        # systemd-boot is shim's second stage (grubx64.efi).
        for directory in ("EFI/BOOT", "EFI/carrel"):
            for name in ("grubx64.efi", "mmx64.efi"):
                path = boot / directory / name
                expect(path.is_file() and path.stat().st_size > 50 * 1024,
                       f"ESP is missing {directory}/{name}", failures)
                expect(path.is_file() and pe_is_signed(path),
                       f"{directory}/{name} is not signed", failures)
        shim = boot / "EFI/carrel/shimx64.efi"
        expect(shim.is_file() and shim.stat().st_size > 500 * 1024,
               "ESP is missing EFI/carrel/shimx64.efi", failures)
        expect(shim.is_file() and pe_is_signed(shim), "shim is not signed", failures)
        expect((boot / "EFI/BOOT/BOOTX64.EFI").is_file()
               and (boot / "EFI/BOOT/BOOTX64.EFI").read_bytes() == shim.read_bytes(),
               "EFI/BOOT/BOOTX64.EFI is not shim", failures)
        expect((boot / "EFI/BOOT/grubx64.efi").is_file()
               and (boot / "EFI/BOOT/grubx64.efi").read_bytes()
               == (boot / "EFI/carrel/grubx64.efi").read_bytes(),
               "the removable-media second stage differs from EFI/carrel", failures)
        expect(not any((boot / "EFI/Linux").glob("*.efi")) if (boot / "EFI/Linux").exists()
               else True, "the ESP still has self-signed UKIs", failures)
        for name in ("carrel-a.conf", "carrel-recovery.conf"):
            expect((boot / "loader/entries" / name).is_file(),
                   f"ESP is missing the {name} boot entry", failures)
        loader = (boot / "loader/loader.conf").read_text()
        expect("default carrel-a.conf" in loader, "loader.conf does not default to slot A",
               failures)
        expect("editor no" in loader, "loader.conf does not disable the editor", failures)
        grub = (boot / "grub/grub.cfg").read_text()
        expect("Carrel recovery" in grub, "GRUB menu has no recovery entry", failures)
        expect("carrel_tries" in grub and "save_env --file /grub/grubenv" in grub,
               "GRUB menu has no boot counter", failures)
        expect("$((" not in grub, "GRUB menu uses unsupported arithmetic", failures)
        expect("roothash=${carrel_a_roothash}" in grub,
               "GRUB slot A has no verity root hash", failures)
        expect("roothash=${carrel_b_roothash}" in grub,
               "GRUB slot B has no verity root hash", failures)
        kernels = list((boot / "carrel").glob("*/vmlinuz"))
        expect(bool(kernels), "ESP has no kernel for the boot entries", failures)
        expect(any((kernel.parent / "initrd").is_file() for kernel in kernels),
               "the ESP has no per-kernel initrd for the update transfer", failures)
        # The kernel is Debian's signed kernel: shim verifies it for
        # systemd-boot.
        expect(all(pe_is_signed(kernel) for kernel in kernels),
               "the kernel is not signed", failures)

        entry = boot / "loader/entries/carrel-a.conf"
        cmdline = entry_options(entry)
        expect("roothash=" in cmdline, "slot A entry has no roothash=", failures)
        expect("systemd.verity_root_data=PARTUUID=11111111" in cmdline,
               "slot A entry does not name the verity data partition", failures)
        expect("systemd.verity_root_hash=PARTUUID=55555555" in cmdline,
               "slot A entry does not name the verity hash partition", failures)
        expect(entry_value(entry, "linux").endswith("/vmlinuz"),
               "slot A entry has no kernel", failures)
        grubenv = work / "grubenv"
        shutil.copyfile(boot / "grub/grubenv", grubenv)
        env = subprocess.run(
            ["grub-editenv", grubenv, "list"],
            check=True, capture_output=True, text=True,
        ).stdout
        expect("carrel_a_roothash=" in env and "carrel_b_roothash=" in env,
               "grubenv has no per-slot verity root hashes", failures)

    # Root-A: the real product.
    extracted = out / "test" / "image-check" / "root-a"
    if extracted.exists():
        shutil.rmtree(extracted)
    extracted.parent.mkdir(parents=True, exist_ok=True)
    extract_erofs(image, root_a, extracted)

    def root_file(path: str) -> Path:
        return extracted / path.lstrip("/")

    for binary, minimum in (
        ("/usr/bin/carrel-shell", 500 * 1024),
        ("/usr/bin/carreld", 5 * 1024**2),
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
    expect(b"libgtksourceview-5" in root_file("/usr/bin/carrel-shell").read_bytes(),
           "carrel-shell does not link GtkSourceView 5", failures)

    for path in (
        "/usr/lib/systemd/system/carreld.service",
        "/usr/share/dbus-1/system.d/page.nostos.Carrel.Daemon1.conf",
        "/usr/lib/carrel/installer",
        "/usr/lib/carrel/recovery",
        "/usr/lib/carrel/apply-update",
        "/usr/lib/carrel/boot-complete",
        "/usr/lib/carrel/data-init",
        "/usr/lib/carrel/grow-data",
        "/usr/lib/carrel/sysupdate/root.transfer",
        "/usr/lib/carrel/sysupdate/verity.transfer",
        "/usr/lib/carrel/sysupdate/sig.transfer",
        "/usr/lib/carrel/sysupdate/entry-a.transfer",
        "/usr/lib/carrel/sysupdate/entry-b.transfer",
        "/usr/lib/carrel/sysupdate/kernel.transfer",
        "/usr/lib/carrel/repart-grow/50-data.conf",
        "/usr/lib/carrel/update-url",
        "/usr/lib/systemd/import-pubring.gpg",
        "/usr/share/carrel/keys/verity.crt",
        # Time zone data: without it every settings.time_zone falls back.
        "/usr/share/zoneinfo/Europe/Stockholm",
        "/usr/share/zoneinfo/America/New_York",
        "/usr/lib/systemd/system/carrel-online-update-test.service",
        "/usr/lib/carrel/online-update-test",
    ):
        expect(root_file(path).exists(), f"missing {path}", failures)
    installer = root_file("/usr/lib/carrel/installer").read_text()
    expect("Start Carrel" in installer and "Install Carrel" in installer,
           "installer menu does not offer Start Carrel / Install Carrel", failures)
    expect("Carrel runs from the external drive and saves your writing there." in installer,
           "installer menu is missing Start Carrel explanation", failures)
    expect("Carrel copies itself to an internal disk and erases that disk." in installer,
           "installer menu is missing Install Carrel explanation", failures)
    expect("USB stick" not in installer and "Try Carrel" not in installer,
           "installer still mentions USB stick or Try Carrel", failures)
    update_url = root_file("/usr/lib/carrel/update-url").read_text().strip()
    expect(update_url.startswith("http"), f"update URL is not a URL: {update_url!r}", failures)
    expect(root_file("/usr/lib/systemd/import-pubring.gpg").stat().st_size > 100,
           "the update public keyring is empty", failures)

    enabled = root_file("/etc/systemd/system/multi-user.target.wants/carreld.service")
    expect(enabled.is_symlink() or enabled.exists(), "carreld.service is not enabled", failures)

    session = root_file("/etc/systemd/system/carrel-session.service").read_text()
    exec_line = next(
        (line.strip() for line in session.splitlines() if line.startswith("ExecStart=")),
        "",
    )
    expect(
        re.fullmatch(
            r"ExecStart=/usr/bin/cage(?: -[a-z]+)* -- /usr/bin/carrel-shell",
            exec_line,
        ) is not None,
        f"session does not start cage with carrel-shell: {exec_line!r}",
        failures,
    )
    expect(" -s" not in exec_line, "session passes cage -s (VT switching)", failures)
    expect("Restart=always" in session, "session does not restart", failures)
    expect("User=writer" in session, "session does not run as writer", failures)
    # Virtual GPUs (VirtualBox/VMware SVGA) refuse or crash cage's GL renderer;
    # session-env and session-check switch the session to software drawing.
    expect("ExecStartPre=+/usr/lib/carrel/session-env" in session,
           "session does not choose its renderer before start", failures)
    expect("EnvironmentFile=-/run/carrel/session.env" in session,
           "session does not read the renderer choice", failures)
    expect("ExecStopPost=+/usr/lib/carrel/session-check" in session,
           "session does not fall back to software after a renderer failure", failures)
    for helper in (
        "/usr/lib/carrel/session-env",
        "/usr/lib/carrel/session-check",
        "/usr/lib/carrel/keyboard-layout",
        "/usr/bin/loadkeys",
    ):
        path = root_file(helper)
        expect(path.is_file() and path.stat().st_mode & 0o111 != 0,
               f"{helper} is missing or not executable", failures)

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

    # Spellcheck: libspelling and one dictionary per offered language
    # (shell/src/spell.rs).  Several of the files are relative symlinks.
    expect(any(root_file("/usr/lib/x86_64-linux-gnu").glob("libspelling-1.so.*")),
           "libspelling is not installed", failures)
    for code in ("en_US", "de_DE", "fr_FR", "es_ES", "it_IT", "pt_PT", "nl_NL",
                 "pl_PL", "tr_TR"):
        for suffix in ("aff", "dic"):
            path = root_file(f"/usr/share/hunspell/{code}.{suffix}")
            expect(path.is_file() and path.stat().st_size > 1000,
                   f"spelling dictionary {code}.{suffix} is missing", failures)

    fstab = root_file("/etc/fstab").read_text()
    expect("PARTUUID=33333333-3333-4333-8333-333333333333 /data" in fstab,
           "fstab does not mount data", failures)
    expect("PARTUUID=44444444-4444-4444-8444-444444444444 /boot" in fstab,
           "fstab does not mount the ESP", failures)

    passwd = root_file("/etc/passwd").read_text()
    expect("writer:x:1000:1000:" in passwd, "writer user is missing", failures)
    writing = root_file("/data/writing")
    expect(writing.is_dir(), "/data/writing is missing", failures)

    oauth = root_file("/usr/share/carrel/oauth.toml")
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
        "payload/carrel-root-0.3.0.raw",
        "payload/carrel-verity-0.3.0.raw",
        "payload/carrel-verity-sig-0.3.0.sig",
        "payload/carrel-b-0.3.0.conf",
        "README",
    ):
        expect((update_dir / name).exists(), f"missing update fixture {name}", failures)

    # Verify the actual verity tree, not just that the partitions exist.
    work = out / "test" / "verity-check"
    work.mkdir(parents=True, exist_ok=True)
    with mounted(image, esp, "vfat", root=out / "test") as boot:
        cmdline = entry_options(boot / "loader/entries/carrel-a.conf")
        root_b_hash = grubenv_get(boot / "grub/grubenv", "carrel_b_roothash")
    match = re.search(r"roothash=([0-9a-f]{64})", cmdline)
    if not match:
        failures.append("cannot read the root hash from the slot A entry")
    else:
        with loop_device(image, root_a) as data_dev, loop_device(image, verity_a) as hash_dev:
            result = subprocess.run(
                [tool("veritysetup"), "verify", data_dev, hash_dev, match.group(1)],
                capture_output=True, text=True,
            )
            expect(result.returncode == 0, "root-a verity verification failed", failures)
        with loop_device(image, root_b) as data_dev, loop_device(image, verity_b) as hash_dev:
            result = subprocess.run(
                [tool("veritysetup"), "verify", data_dev, hash_dev, root_b_hash],
                capture_output=True, text=True,
            )
            expect(result.returncode == 0, "root-b verity verification failed", failures)
        for label, part in (("a", sig_a), ("b", sig_b)):
            with loop_device(image, part) as dev:
                with open(dev, "rb") as signature:
                    text = signature.read(8192)
            expect(b'"rootHash"' in text and b'"signature"' in text,
                   f"verity signature partition {label} is not signed JSON", failures)

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
        # boot-complete reads the SecureBoot EFI variable the kernel exposes.
        # Without Secure Boot the firmware has no SecureBoot variable at all.
        if qemu.secure:
            expect("CARREL-BOOT: secureboot=1" in text,
                   "the firmware's Secure Boot state is not 1", failures)
        else:
            expect("CARREL-BOOT: secureboot=1" not in text,
                   "Secure Boot is unexpectedly on", failures)
        if qemu.secure:
            expect("EFI stub: UEFI Secure Boot is enabled." in text,
                   "the EFI stub did not see Secure Boot", failures)
            expect("secureboot: Secure boot enabled" in text,
                   "the kernel did not report Secure Boot enabled", failures)
            expect("Kernel is locked down from EFI Secure Boot" in text,
                   "the kernel is not locked down by Secure Boot", failures)
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
    expect(text.count("Started carrel-session.service") == 1,
           "carrel-session started more than once", failures)
    expect("Reached target graphical.target" in text,
           "graphical.target was not reached", failures)
    expect("Started carreld.service" in text, "carreld did not start", failures)
    expect("Finished carrel-rfkill.service" in text, "carrel-rfkill did not run", failures)
    expect("Finished carrel-grow-data.service" in text,
           "carrel-grow-data did not finish", failures)
    expect("CARREL-DATA: /var is on data" in text, "/var is not on data", failures)
    expect("CARREL-DATA: /etc overlay is on data" in text,
           "/etc overlay is not on data", failures)
    expect(re.search(r"CARREL-BOOT: slot [ab] marked good", text) is not None,
           "the running slot was not marked good (boot-complete failed)", failures)
    if failures:
        raise BootError("; ".join(failures))


def boot_to_editor(qemu: Qemu, timeout: float, *, first_run: bool = False) -> None:
    qemu.wait(r"Started carrel-session\.service", timeout)
    # Mark-good is pulled in by graphical.target and ordered after the session,
    # so it logs before the target itself is reached.
    qemu.wait(r"CARREL-BOOT: slot [ab] marked good", timeout)
    qemu.wait(r"Reached target graphical\.target", timeout)
    time.sleep(3)  # let GTK draw the first frame
    if first_run:
        # Language, keyboard, time zone, "Connect sync? Not now": answer each
        # step with Enter, but look at the screen first.  A key sent while the
        # shell is still busy can be lost, and a blind count of four then
        # leaves the device on a first-run step instead of the editor.
        with tempfile.TemporaryDirectory(prefix="first-run-") as scratch:
            shot = Path(scratch) / "screen.png"
            for _ in range(12):
                qemu.screenshot(shot)
                text = ocr(shot)
                if not re.search(
                        r"short answers|Language|Keyboard|Time zone|Connect sync",
                        text):
                    break
                qemu.key("ret")
                time.sleep(1.5)
            else:
                raise BootError("first run did not finish after 12 answers")
        time.sleep(4)
    time.sleep(2)


# --------------------------------------------------------------------------
# Scenarios


def qcow2_overlay(base: Path, path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.unlink(missing_ok=True)
    run("qemu-img", "create", "-q", "-f", "qcow2", "-F", "raw",
        "-b", base.resolve(), path.resolve())
    return path


def scenario_boot(mode: str, image: Path, out: Path, timeout: float,
                  secure: bool = False) -> None:
    label = "secureboot" if secure else mode
    overlay = qcow2_overlay(image, out / "test" / f"boot-{label}" / "disk.qcow2")
    with Qemu(f"boot-{label}", out, mode,
              [{"path": overlay, "format": "qcow2"}], secure=secure) as qemu:
        boot_to_editor(qemu, timeout, first_run=True)
        runtime_check(qemu)
        # Type one line so the screenshot shows the real editor (and proves
        # that QMP keyboard input reaches carrel-shell).
        qemu.type_text("The quiet morning")
        time.sleep(1.5)
        shot = qemu.screenshot(out / f"screenshot-{label}.png")
        # QEMU has no battery.  The old image showed a hard-coded 76 %; the
        # real daemon path must show no percentage at all.
        text = ocr(shot)
        if re.search(r"\d+\s*%", text):
            raise BootError(f"[{label}] bottom line still shows a battery value:\n{text}")
        print(f"{label}: PASS (no battery percentage in screenshot)", flush=True)


def wait_installer(qemu: Qemu, marker: str, timeout: float = 600.0) -> None:
    qemu.wait(rf"CARREL-INSTALLER: {re.escape(marker)}", timeout)


def scenario_install(image: Path, out: Path, timeout: float,
                     secure: bool = False) -> Path:
    label = "install-secureboot" if secure else "install"
    base = out / "test" / label
    base.mkdir(parents=True, exist_ok=True)
    source = qcow2_overlay(image, base / "usb.qcow2")
    target = base / "disk.img"
    if target.exists():
        target.unlink()
    run("qemu-img", "create", "-q", "-f", "raw", target, "16G")

    with Qemu(label, out, "uefi", [{"path": target, "format": "raw"}],
              usb={"path": source, "format": "qcow2"}, secure=secure) as qemu:
        wait_installer(qemu, "main menu", timeout)
        qemu.screenshot(out / f"screenshot-{label}-menu.png", light=False)
        qemu.key("down")
        qemu.key("ret")
        wait_installer(qemu, "disk menu", 120)
        qemu.key("ret")
        wait_installer(qemu, "erase prompt", 120)
        qemu.type_text("ERASE")
        qemu.key("ret")
        wait_installer(qemu, "installing", 120)
        wait_installer(qemu, "complete", timeout)
        installer_log = qemu.text()
        qemu.stop()

    # The installer must have made Carrel the first UEFI boot entry.
    match = re.search(r"UEFI boot entry Carrel is first \(Boot([0-9A-Fa-f]{4})\)",
                      installer_log)
    if not match:
        raise BootError("the installer did not register a Carrel UEFI boot entry")
    entry = match.group(1).upper()
    if not re.search(rf"BootOrder: {entry}\b", installer_log):
        raise BootError(f"Boot{entry} (Carrel) is not first in BootOrder")
    if not re.search(rf"Boot{entry}\* Carrel.*\\EFI\\carrel\\shimx64\.efi",
                     installer_log, re.IGNORECASE):
        raise BootError("the Carrel boot entry does not point at shim")

    parts = read_gpt(target)
    data = part_named(parts, "carrel-data")
    if data["size"] < 10 * 1024**3:
        raise BootError(f"installer did not grow data: {data['size'] // 1024**3} GiB")

    # Boot the installed disk without the USB stick, with the firmware
    # variables the installer wrote (so the Carrel entry is used).
    installed = "installed-secureboot" if secure else "installed"
    nvram = out / "test" / label / "OVMF_VARS.fd"
    with Qemu(installed, out, "uefi", [{"path": target, "format": "raw"}],
              secure=secure, vars_from=nvram) as qemu:
        boot_to_editor(qemu, timeout, first_run=True)
        runtime_check(qemu)
        qemu.type_text("The quiet morning")
        time.sleep(1.5)
        qemu.screenshot(out / ("screenshot-install-secureboot.png" if secure
                               else "screenshot-install.png"))
        qemu.powerdown()
    print(f"{label}: PASS", flush=True)
    return target


# Writing seeded for the recovery copy.  ext4 allows all of these names; FAT
# and exFAT reject some characters and ignore case, so the copy renames them.
RECOVERY_PLAIN_NAMES = (
    "Recovery note.md",
    "Anteckning om sjön åäö.md",
    "Chapters/Part one.md",
)
RECOVERY_HARD_NAMES = (
    'What is "truth" <now> | *maybe*.md',
    "Draft?.md",
    "Draft-.md",
    "Time: 12.md",
    "Case.md",
    "case.md",
    "Ideas./Inside.md",
)


def _recovery_copy(disk: Path, usb: Path, out: Path, timeout: float,
                   fstype: str, *, reset: bool) -> None:
    """Boot the recovery menu, copy to the USB drive, optionally reset."""
    name = f"recovery-{fstype}"
    with Qemu(name, out, "uefi", [{"path": disk, "format": "raw"}],
              usb={"path": usb, "format": "raw"}) as qemu:
        qemu.wait(r"CARREL-RECOVERY: menu", timeout)
        time.sleep(3)  # whiptail draws over the last kernel console lines
        qemu.screenshot(out / f"screenshot-{name}.png", light=False)
        if "Started carrel-session.service" in qemu.text():
            raise BootError("the session started while booted into recovery")
        # Menu: Boot previous, Copy writing, Reset settings, Shut down.
        qemu.key("down")
        qemu.key("ret")
        time.sleep(2)  # the disk menu is drawn by whiptail
        qemu.key("ret")
        qemu.wait(r"CARREL-RECOVERY: (copied to|copy failed|could not mount)", 180)
        time.sleep(1)
        qemu.screenshot(out / f"screenshot-{name}-copy.png", light=False)
        if "CARREL-RECOVERY: copied to" not in qemu.text():
            raise BootError(f"recovery copy to {fstype} failed\n{qemu.serial_tail(30)}")
        if reset:
            qemu.key("ret")  # close the "Writing copied" box
            time.sleep(2)
            qemu.key("down")
            qemu.key("down")
            qemu.key("ret")
            time.sleep(2)
            qemu.screenshot(out / "screenshot-recovery-reset.png", light=False)
            qemu.key("ret")  # Yes
            qemu.wait(r"CARREL-RECOVERY: settings reset", 60)
            time.sleep(1)
            qemu.key("ret")  # close the "Settings were reset" box
            time.sleep(2)
            for _ in range(3):
                qemu.key("down")
            qemu.key("ret")  # Shut down, which flushes /data
            qemu.wait(r"CARREL-RECOVERY: shut down", 60)
            qemu.proc.wait(timeout=90)
        qemu.stop()


def _check_recovery_copy(usb: Path, part: dict, out: Path, fstype: str,
                         seeded: dict[str, str]) -> None:
    options = USB_FILESYSTEMS[fstype][2]
    with mounted(usb, part, fstype, root=out / "test",
                 extra_options=options) as root:
        copies = list(root.glob("carrel-writing-*"))
        if len(copies) != 1:
            raise BootError(f"{fstype}: expected one copy folder, found {copies}")
        copy = copies[0]
        found = {
            str(path.relative_to(copy)): path.read_bytes().decode(errors="replace")
            for path in copy.rglob("*") if path.is_file()
        }
    exact = seeded if fstype == "ext4" else {
        name: text for name, text in seeded.items()
        if name in RECOVERY_PLAIN_NAMES
        or name in (".carrel/settings.json", ".carrel/stats.json")
    }
    wrong = [name for name, text in exact.items() if found.get(name) != text]
    if wrong:
        raise BootError(f"{fstype}: missing or changed on the drive: {wrong}; "
                        f"found {sorted(found)}")
    # Renamed files keep their contents and never overwrite each other.
    lost = [name for name, text in seeded.items() if text not in found.values()]
    if lost:
        raise BootError(f"{fstype}: contents lost for {lost}; found {sorted(found)}")


def scenario_recovery(disk: Path, out: Path, timeout: float) -> None:
    parts = read_gpt(disk)
    esp = part_typed(parts, ESP_GUID)
    data = part_named(parts, "carrel-data")
    loader_path = "loader/loader.conf"
    seeded = {
        name: f"{name}\n\nThis must reach the USB drive.\n"
        for name in RECOVERY_PLAIN_NAMES + RECOVERY_HARD_NAMES
    }
    seeded[".carrel/settings.json"] = '{"theme": "dark"}\n'
    seeded[".carrel/stats.json"] = '{"words": 1}\n'
    seeded[".carrel/trash/recovery/Old?.md"] = "A deleted draft.\n"
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        for name, text in seeded.items():
            path = root / "writing" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    filesystems = [
        fstype for fstype, (_, mkfs, _) in USB_FILESYSTEMS.items()
        if Path(tool(mkfs[0])).exists()
    ]
    with mounted(disk, esp, "vfat", writable=True, root=out / "test") as boot:
        loader = boot / loader_path
        original = loader.read_text()
        loader.write_text("default carrel-recovery.conf\ntimeout 2\neditor no\n")
    failures: list[str] = []
    try:
        for fstype in filesystems:
            usb = out / "test" / "recovery" / f"usb-{fstype}.img"
            part = make_usb_disk(usb, fstype)
            # Reset last: the earlier copies must still see the settings.
            reset = fstype == filesystems[-1]
            try:
                _recovery_copy(disk, usb, out, timeout, fstype, reset=reset)
                _check_recovery_copy(usb, part, out, fstype, seeded)
                print(f"recovery: {fstype} copy ok", flush=True)
            except BootError as exc:
                failures.append(str(exc))
                print(f"recovery: {fstype} FAILED: {exc}", flush=True)
    finally:
        with mounted(disk, esp, "vfat", writable=True, root=out / "test") as boot:
            (boot / loader_path).write_text(original)
    if failures:
        raise BootError("; ".join(failures))

    with mounted(disk, data, "ext4", root=out / "test") as root:
        writing = root / "writing"
        for name in (".carrel/settings.json", ".carrel/stats.json"):
            if (writing / name).exists():
                raise BootError(f"reset settings left {name}")
        for name, text in seeded.items():
            if name in (".carrel/settings.json", ".carrel/stats.json"):
                continue
            path = writing / name
            if not path.is_file() or path.read_text() != text:
                raise BootError(f"reset settings damaged {name}")
    print(f"recovery: PASS (copied to {', '.join(filesystems)}; settings reset)",
          flush=True)


def scenario_powercut(disk: Path, out: Path, timeout: float) -> None:
    with Qemu("powercut", out, "uefi", [{"path": disk, "format": "raw"}]) as qemu:
        # In an "all" run the disk comes from the recovery scenario, which ends
        # with "Reset settings", so this boot can start at First run again.
        boot_to_editor(qemu, timeout, first_run=True)
        qemu.type_text("hello power cut")
        time.sleep(2.5)  # journal fdatasync is at most 1 s behind
        qemu.reset()
        qemu.wait_fresh(r"carrel\.slot=a", timeout)
        qemu.wait(r"Started carrel-session\.service", timeout)
        time.sleep(8)  # replay the journal and autosave after 5 s idle
        qemu.screenshot(out / "screenshot-powercut-recovered.png")
        qemu.powerdown()

    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    found = []
    with mounted(disk, data, "ext4", root=out / "test") as root:
        for path in (root / "writing").rglob("*"):
            if path.is_file():
                found.append(path.read_text(errors="replace"))
    if not any("hello power cut" in text for text in found):
        raise BootError("text did not survive the power cut in /data/writing")
    print("powercut: PASS", flush=True)


def scenario_keyboard(image: Path, out: Path, timeout: float) -> None:
    """Switch Keyboard layout to Swedish in Settings and type å ä ö via QMP."""
    disk = out / "test" / "keyboard" / "disk.img"
    disk.parent.mkdir(parents=True, exist_ok=True)
    disk.unlink(missing_ok=True)
    run("cp", "--sparse=always", "--reflink=auto", image, disk)

    with Qemu("keyboard", out, "uefi", [{"path": disk, "format": "raw"}]) as qemu:
        boot_to_editor(qemu, timeout, first_run=True)
        runtime_check(qemu)
        # Type before changing layout to verify the open document is saved
        # before the session reloads.
        qemu.type_text("Before ")
        time.sleep(0.8)

        # Open Settings (Ctrl+,), jump to Keyboard layout (End, then 4× Up),
        # cycle 8 steps from US to Swedish, and leave with Esc to reload cage.
        qemu.key("ctrl-comma")
        time.sleep(1.5)
        qemu.key("end")
        time.sleep(0.3)
        for _ in range(4):
            qemu.key("up")
            time.sleep(0.2)
        for _ in range(8):
            qemu.key("right")
            time.sleep(0.2)
        time.sleep(0.5)
        qemu.key("esc")

        # Session restarts cleanly so cage picks up XKB_DEFAULT_LAYOUT=se.
        # systemd stops echoing unit status lines to ttyS0 after boot, so
        # wait until the restarted editor has reopened "Before".
        time.sleep(3)
        with tempfile.TemporaryDirectory(prefix="keyboard-reload-") as scratch:
            shot = Path(scratch) / "screen.png"
            deadline = time.monotonic() + min(timeout, 90.0)
            while True:
                qemu.screenshot(shot)
                text = ocr(shot)
                if "Before" in text and "Settings" not in text:
                    break
                if time.monotonic() > deadline:
                    raise BootError(
                        f"[keyboard] editor did not reopen after reload; OCR:\n{text}"
                    )
                time.sleep(1.5)
        time.sleep(1.5)

        # Physical US keys [ ' ; map to å ä ö under the Swedish XKB layout.
        for key in (
            "bracket_left",
            "apostrophe",
            "semicolon",
            "spc",
            "shift-bracket_left",
            "shift-apostrophe",
            "shift-semicolon",
        ):
            qemu.key(key)
            time.sleep(0.15)
        time.sleep(6)  # autosave flushes after 5 s idle
        qemu.screenshot(out / "screenshot-keyboard-se.png")
        qemu.powerdown()

    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    saved_keyboard = None
    found: list[str] = []
    with mounted(disk, data, "ext4", root=out / "test") as root:
        settings_file = root / "writing" / ".carrel" / "settings.json"
        if settings_file.is_file():
            saved_keyboard = json.loads(settings_file.read_text()).get("keyboard")
        for path in (root / "writing").rglob("*.md"):
            if path.is_file():
                found.append(path.read_text(errors="replace"))
    if saved_keyboard != "se":
        raise BootError(f"expected saved keyboard 'se', got {saved_keyboard!r}")
    if not any("Before " in text and "åäö ÅÄÖ" in text for text in found):
        raise BootError(
            f"Swedish characters åäö ÅÄÖ did not reach /data/writing; found: {found!r}"
        )
    print("keyboard: PASS (Swedish layout produced åäö ÅÄÖ)", flush=True)


def seed_shutdown_sync(disk: Path, url: str, out: Path) -> None:
    """Sync at shutdown on, a paired server, and a wired connection to it."""
    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        settings = root / "writing" / ".carrel" / "settings.json"
        settings.parent.mkdir(parents=True, exist_ok=True)
        settings.write_text(json.dumps(
            {"sync_at_shutdown": True, "first_run_complete": True}))
        run("chown", "-R", "1000:1000", root / "writing")
        sync = root / ".carrel" / "sync.toml"
        sync.parent.mkdir(parents=True, exist_ok=True)
        sync.write_text(f'server_url = "{url}"\ntoken = "boot-test"\n')
        sync.chmod(0o600)
        # data/etc is the upper layer of /etc.  The image makes no wired
        # connection by itself (no-auto-default).
        wired = (root / "etc" / "NetworkManager" / "system-connections"
                 / "boot-test.nmconnection")
        wired.parent.mkdir(parents=True, exist_ok=True)
        wired.write_text("[connection]\nid=boot-test\ntype=ethernet\n")
        wired.chmod(0o600)


def scenario_shutdown_sync(image: Path, out: Path, timeout: float) -> None:
    disk = out / "test" / "shutdown-sync" / "disk.img"
    disk.parent.mkdir(parents=True, exist_ok=True)
    run("cp", "--sparse=always", image, disk)

    # A listening socket nobody accepts from: TCP connects, TLS never answers.
    with socket.socket() as silent:
        silent.bind(("0.0.0.0", 0))
        silent.listen(8)
        port = silent.getsockname()[1]
        seed_shutdown_sync(disk, f"https://10.0.2.2:{port}", out)
        with Qemu("shutdown-sync", out, "uefi",
                  [{"path": disk, "format": "raw"}], network=True) as qemu:
            boot_to_editor(qemu, timeout)
            time.sleep(10)  # NetworkManager brings the wired link up
            assert qemu.qmp is not None and qemu.proc is not None
            # Power button soon after the last key: the 5 s autosave has not
            # run, so only the flush on PrepareForShutdown can save it in time.
            qemu.type_text("last words")
            time.sleep(1)  # let the shell receive every key
            started = time.monotonic()
            qemu.qmp.command("system_powerdown")
            time.sleep(4)
            qemu.screenshot(out / "screenshot-shutdown-sync.png")
            try:
                qemu.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                raise BootError(
                    "power off hung with sync at shutdown on\n"
                    f"--- serial tail ---\n{qemu.serial_tail()}")
            elapsed = time.monotonic() - started

    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    with mounted(disk, data, "ext4", root=out / "test") as root:
        journal = subprocess.run(
            ["journalctl", "--directory", str(root / "var" / "log" / "journal"),
             "--unit", "carreld.service", "--output", "cat", "--no-pager"],
            check=True, capture_output=True, text=True,
        ).stdout
        # The commit carreld makes right before the sync.
        history = root / "writing" / ".carrel" / "history.git"
        before_sync = subprocess.run(
            ["git", "-c", "safe.directory=*", "--git-dir", str(history), "log",
             "--all", "--patch", "--format=%s", "--grep", "before sync"],
            check=True, capture_output=True, text=True,
        ).stdout
    if "last words" not in before_sync:
        raise BootError(
            "the text typed before power off is not in the pre-sync commit:\n"
            + before_sync)
    (out / "test" / "shutdown-sync" / "carreld.log").write_text(journal)
    journal = clean_serial(journal)
    for line in ("sync at shutdown: starting", "sync at shutdown: timed out"):
        if line not in journal:
            raise BootError(
                f"carreld did not log {line!r}\n--- journal ---\n{journal}")
    # The 12 s limit passed, and the rest of the shutdown is quick.
    if elapsed < 12:
        raise BootError(f"power off took {elapsed:.1f} s; it did not wait for the sync")
    print(f"shutdown-sync: PASS (power off took {elapsed:.1f} s)", flush=True)


def palette_run(qemu: Qemu, query: str) -> None:
    """Ctrl+K, a few letters, Enter."""
    qemu.key("ctrl-k")
    time.sleep(1)
    qemu.type_text(query)
    time.sleep(0.5)
    qemu.key("ret")


def wait_status(qemu: Qemu, wanted: str, timeout: float = 90.0) -> None:
    assert qemu.qmp is not None
    deadline = time.monotonic() + timeout
    while True:
        status = qemu.qmp.command("query-status")["status"]
        if status == wanted:
            return
        if time.monotonic() > deadline:
            raise BootError(
                f"[{qemu.name}] VM is {status!r}, not {wanted!r}\n"
                f"--- serial tail ---\n{qemu.serial_tail()}")
        time.sleep(0.5)


def scenario_palette_power(image: Path, out: Path, timeout: float) -> None:
    disk = out / "test" / "palette-power" / "disk.img"
    disk.parent.mkdir(parents=True, exist_ok=True)
    run("cp", "--sparse=always", image, disk)

    # A server that never answers, so Sync at shutdown has something to try.
    with socket.socket() as silent:
        silent.bind(("0.0.0.0", 0))
        silent.listen(8)
        seed_shutdown_sync(disk, f"https://10.0.2.2:{silent.getsockname()[1]}", out)
        with Qemu("palette-power", out, "uefi",
                  [{"path": disk, "format": "raw"}], network=True) as qemu:
            assert qemu.proc is not None
            boot_to_editor(qemu, timeout)
            time.sleep(10)  # NetworkManager brings the wired link up

            # Each action runs well inside the 5 s autosave delay, so only the
            # save the palette makes can have the words on disk in time.
            qemu.type_text("before suspend. ")
            palette_run(qemu, "suspend")
            wait_status(qemu, "suspended")
            time.sleep(3)
            assert qemu.qmp is not None
            qemu.qmp.command("system_wakeup")
            wait_status(qemu, "running")
            time.sleep(8)
            qemu.screenshot(out / "screenshot-palette-resumed.png")

            qemu.type_text("before restart. ")
            qemu.pos = len(qemu.text())
            palette_run(qemu, "restart")
            qemu.wait(r"carrel\.slot=a", timeout)
            boot_to_editor(qemu, timeout)
            time.sleep(10)
            qemu.screenshot(out / "screenshot-palette-restarted.png")

            qemu.type_text("before shut down.")
            palette_run(qemu, "shut down")
            try:
                qemu.proc.wait(timeout=60)
            except subprocess.TimeoutExpired:
                raise BootError(
                    "Shut down from the palette did not power the VM off\n"
                    f"--- serial tail ---\n{qemu.serial_tail()}")
            if qemu.proc.returncode != 0:
                raise BootError(f"QEMU exited with {qemu.proc.returncode}")

    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    with mounted(disk, data, "ext4", root=out / "test") as root:
        journal = subprocess.run(
            ["journalctl", "--directory", str(root / "var" / "log" / "journal"),
             "--unit", "carreld.service", "--output", "cat", "--no-pager"],
            check=True, capture_output=True, text=True,
        ).stdout
        history = root / "writing" / ".carrel" / "history.git"
        commits = {
            reason: subprocess.run(
                ["git", "-c", "safe.directory=*", "--git-dir", str(history), "log",
                 "--all", "--patch", "--format=%s", "--grep", reason],
                check=True, capture_output=True, text=True,
            ).stdout
            for reason in ("before suspend", "before restart", "before shut down")
        }
        saved = [path.read_text(errors="replace")
                 for path in (root / "writing").rglob("*.md")]
    (out / "test" / "palette-power" / "carreld.log").write_text(journal)
    journal = clean_serial(journal)
    wanted = "before suspend. before restart. before shut down."
    if not any(wanted in text for text in saved):
        raise BootError(f"expected {wanted!r} in /data/writing; found: {saved!r}")
    # The commit message is also the typed text, so look at the added lines.
    for reason, patch in commits.items():
        if not re.search(rf"(?m)^\+.*{reason}\.", patch):
            raise BootError(f"no {reason!r} commit with those words:\n{patch}")
    # Once for the restart, once for the shut down; never for the suspend.
    syncs = journal.count("sync at shutdown: starting")
    if syncs != 2:
        raise BootError(
            f"sync at shutdown started {syncs} times, not 2\n--- journal ---\n{journal}")
    print("palette-power: PASS (suspend, restart, shut down)", flush=True)


# --------------------------------------------------------------------------
# Typing on a slow USB drive

# A cheap USB 2.0 stick: writes are slow and few per second, reads are fine.
SLOW_STICK = {"bps": 0, "bps_rd": 20_000_000, "bps_wr": 2_000_000,
              "iops": 0, "iops_rd": 0, "iops_wr": 12}


class FakeSyncServer:
    """Enough of spec/02-sync-protocol.md to give carreld real work per round."""

    FILES = 40
    FILE_BYTES = 100_000
    LIBRARY_ITEMS = 1500

    def __init__(self) -> None:
        self.files: dict[str, dict] = {}
        self.seq = 0
        self.rounds = 0
        self.finished = -1
        self.lock = threading.Lock()
        self.requests: list[str] = []
        for index in range(self.FILES):
            self._change(str(uuid.UUID(int=index + 1)), f"Synced/Part {index}.md")

    def _change(self, file_id: str, path: str) -> None:
        self.seq += 1
        previous = self.files.get(file_id)
        line = f"revision {self.seq} of {path}\n"
        content = line * (self.FILE_BYTES // len(line))
        self.files[file_id] = {
            "path": path, "content": content, "seq": self.seq,
            "rev": previous["rev"] + 1 if previous else 1,
        }

    def _entry(self, file_id: str) -> dict:
        item = self.files[file_id]
        return {"file_id": file_id, "path": item["path"], "rev": item["rev"],
                "hash": hashlib.sha256(item["content"].encode()).hexdigest(),
                "deleted": False, "modified_at": int(time.time())}

    def get(self, path: str) -> tuple[int, dict]:
        route, _, query = path.partition("?")
        since = query.partition("=")[2]
        with self.lock:
            if route == "/files":
                changed = [self._entry(i) for i, f in self.files.items()
                           if f["seq"] > int(since or 0)]
                return 200, {"files": changed, "cursor": str(self.seq)}
            if route.startswith("/files/"):
                file_id = route.rsplit("/", 1)[1]
                entry = self._entry(file_id)
                return 200, {**{k: entry[k] for k in
                                ("file_id", "path", "hash", "rev")},
                             "content": self.files[file_id]["content"]}
            if route == "/library":
                if since == str(self.rounds):
                    # The library is the last step of a sync round.
                    self.finished = self.rounds
                    return 200, {"cursor": since}
                items = [{"id": f"item-{n}", "kind": "highlight", "book_id": "b",
                          "text": f"round {self.rounds} quote {n} " * 20}
                         for n in range(self.LIBRARY_ITEMS)]
                return 200, {"books": [{"id": "b", "title": "A book"}],
                             "items": items, "cursor": str(self.rounds)}
        return 404, {"error": "not found"}

    def put(self, path: str, body: dict) -> tuple[int, dict]:
        file_id = path.rsplit("/", 1)[1]
        with self.lock:
            self.seq += 1
            previous = self.files.get(file_id)
            rev = previous["rev"] + 1 if previous else 1
            self.files[file_id] = {"path": body["path"], "content": body["content"],
                                   "seq": self.seq, "rev": rev}
            return 200, {"rev": rev,
                         "hash": hashlib.sha256(body["content"].encode()).hexdigest()}

    def next_round(self) -> None:
        """Change every file, so the next sync pulls and rewrites them all."""
        with self.lock:
            self.rounds += 1
            for file_id, item in list(self.files.items()):
                if item["path"].startswith("Synced/"):
                    self._change(file_id, item["path"])


@contextlib.contextmanager
def fake_sync_server(directory: Path):
    """The fake over HTTPS: yields (server, port, certificate sha256)."""
    key, cert = directory / "server.key", directory / "server.pem"
    run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "30",
        "-subj", "/CN=10.0.2.2", "-addext", "subjectAltName=IP:10.0.2.2",
        "-keyout", key, "-out", cert, stderr=subprocess.DEVNULL)
    der = ssl.PEM_cert_to_DER_cert(cert.read_text())
    server = FakeSyncServer()

    class Handler(http.server.BaseHTTPRequestHandler):
        def reply(self, status: int, body: dict) -> None:
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self) -> None:
            server.requests.append(f"{time.monotonic():.1f} GET {self.path}")
            self.reply(*server.get(self.path))

        def do_PUT(self) -> None:
            server.requests.append(f"{time.monotonic():.1f} PUT {self.path}")
            body = self.rfile.read(int(self.headers["Content-Length"]))
            self.reply(*server.put(self.path, json.loads(body)))

        def do_DELETE(self) -> None:
            self.reply(200, {"rev": 1})

        def log_message(self, *args: object) -> None:
            pass

    httpd = http.server.ThreadingHTTPServer(("0.0.0.0", 0), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    httpd.socket = context.wrap_socket(httpd.socket, server_side=True)
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    try:
        yield server, httpd.server_address[1], hashlib.sha256(der).hexdigest()
    finally:
        httpd.shutdown()
        httpd.server_close()


def type_sentence(qemu: Qemu, number: int, pause: float) -> None:
    """A sentence, then a pause long enough to matter to the autosave."""
    qemu.type_text("the quiet morning came slowly over the hill. ", delay=0.08)
    if number % 2:
        qemu.key("ret")
    time.sleep(pause)


def scenario_typing_slow_disk(image: Path, out: Path, timeout: float) -> None:
    base = out / "test" / "typing-slow-disk"
    base.mkdir(parents=True, exist_ok=True)
    usb = base / "usb.img"
    usb.unlink(missing_ok=True)
    run("cp", "--sparse=always", image, usb)
    run("truncate", "-s", "8G", usb)

    with fake_sync_server(base) as (server, port, fingerprint):
        parts = read_gpt(usb)
        data = part_named(parts, "carrel-data")
        seed_shutdown_sync(usb, f"https://10.0.2.2:{port}", out)
        with mounted(usb, data, "ext4", writable=True, root=out / "test") as root:
            carrel = root / "writing" / ".carrel"
            (carrel / "settings.json").write_text(
                json.dumps({"first_run_complete": True}))
            (carrel / "stall-log").write_text("")
            run("chown", "-R", "1000:1000", root / "writing")
            with (root / ".carrel" / "sync.toml").open("a") as sync:
                sync.write(f'cert_sha256 = "{fingerprint}"\n')

        with Qemu("typing-slow-disk", out, "uefi", [], network=True,
                  usb={"path": usb, "format": "raw"}) as qemu:
            wait_installer(qemu, "main menu", timeout)
            time.sleep(2)
            qemu.key("ret")  # Start Carrel
            wait_installer(qemu, "start selected", 60)
            boot_to_editor(qemu, timeout)
            time.sleep(15)  # let start-up work and the wired link finish
            assert qemu.qmp is not None
            qemu.qmp.command("block_set_io_throttle", device="usb", **SLOW_STICK)

            # Sync off: a new file (named from its first line) and autosaves.
            qemu.key("ctrl-n")
            time.sleep(2)
            qemu.type_text("a slow stick", delay=0.08)
            qemu.key("ret")
            time.sleep(3)
            sentences = 0
            unfinished = 0
            for _ in range(8):
                type_sentence(qemu, sentences, 6.0)
                sentences += 1
            qemu.screenshot(out / "screenshot-typing-slow-disk.png")

            # Typing while sync rounds run.
            for _ in range(2):
                server.next_round()
                qemu.key("ctrl-shift-s")
                time.sleep(2)
                qemu.key("esc")
                time.sleep(1)
                # Type for as long as the round runs, and a little after.
                started = sentences
                while server.finished < server.rounds or sentences - started < 4:
                    if sentences - started >= 40:
                        unfinished += 1
                        qemu.screenshot(out / "screenshot-typing-slow-disk-stuck.png")
                        break
                    type_sentence(qemu, sentences, 3.0)
                    sentences += 1
            qemu.screenshot(out / "screenshot-typing-slow-disk-sync.png")

            qemu.qmp.command("block_set_io_throttle", device="usb",
                             **dict.fromkeys(SLOW_STICK, 0))
            time.sleep(15)
            qemu.powerdown()
        (base / "requests.log").write_text("\n".join(server.requests))

    # The first boot grew the data partition to fill the drive.
    data = part_named(read_gpt(usb), "carrel-data")
    with mounted(usb, data, "ext4", root=out / "test") as root:
        journal = subprocess.run(
            ["journalctl", "--directory", str(root / "var" / "log" / "journal"),
             "--output", "cat", "--no-pager"],
            check=True, capture_output=True, text=True,
        ).stdout
        written = "".join(p.read_text(errors="replace")
                          for p in (root / "writing").glob("*.md"))
    (base / "journal.log").write_text(journal)
    lines = clean_serial(journal).splitlines()
    if "carrel-stall: typing" not in lines:
        raise BootError("the shell logged no typing; the stall log did not run")
    after = lines[lines.index("carrel-stall: typing"):]
    stalls: list[int] = []
    phases: dict[str, list[int]] = {"sync off": [], "during sync": []}
    phase = "sync off"
    for line in after:
        if line == "carrel-stall: sync syncing":
            phase = "during sync"
        elif line.startswith("carrel-stall: sync "):
            phase = "sync off"
        elif match := re.fullmatch(r"carrel-stall: (\d+) ms", line):
            stalls.append(int(match.group(1)))
            phases[phase].append(int(match.group(1)))
    synced = sum("sync finished" in line for line in lines)
    typed = written.count("quiet morning")
    print(f"typing-slow-disk: {synced} sync rounds finished; "
          f"{typed} of {sentences} sentences on disk", flush=True)
    for name, found in phases.items():
        print(f"  {name}: {len(found)} stalls over 50 ms, worst "
              f"{max(found, default=0)} ms, total {sum(found)} ms: {found}",
              flush=True)
    if unfinished:
        raise BootError(f"{unfinished} sync round(s) did not finish in time")
    if synced < 2:
        raise BootError(f"expected two finished sync rounds, saw {synced}")
    if typed != sentences:
        raise BootError(f"expected {sentences} typed sentences on disk, found {typed}")
    if stalls:
        raise BootError(f"the main loop stalled while typing: {stalls}")
    usb.unlink(missing_ok=True)
    print("typing-slow-disk: PASS", flush=True)


def stage_update(disk: Path, update_dir: Path, out: Path) -> None:
    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        payload = root / ".carrel" / "update" / "payload"
        payload.mkdir(parents=True, exist_ok=True)
        for source in sorted((update_dir / "payload").iterdir()):
            destination = payload / source.name
            if destination.exists():
                destination.unlink()
            run("cp", "--sparse=always", source, destination)


def clear_update(disk: Path, out: Path) -> None:
    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        shutil.rmtree(root / ".carrel" / "update", ignore_errors=True)


def prepare_online_update(disk: Path, url: str, out: Path) -> None:
    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        update = root / ".carrel" / "update"
        shutil.rmtree(update, ignore_errors=True)
        update.mkdir(parents=True)
        (update / "test-url").write_text(url + "\n")


def root_version(disk: Path, slot: str, out: Path) -> str:
    parts = read_gpt(disk)
    root_part = part_by_uuid(parts, ROOT_A_UUID if slot == "a" else ROOT_B_UUID)
    destination = out / "test" / f"root-{slot}-version"
    shutil.rmtree(destination, ignore_errors=True)
    extract_erofs(disk, root_part, destination)
    try:
        release = (destination / "usr/lib/os-release").read_text()
    finally:
        shutil.rmtree(destination, ignore_errors=True)
    match = re.search(r"^VERSION_ID=\"?([^\"\n]+)\"?$", release, re.MULTILINE)
    return match.group(1) if match else ""


def scenario_online_update(disk: Path, release_dir: Path, out: Path,
                           timeout: float, expect_version: str = "0.3.0") -> None:
    if not (release_dir / "SHA256SUMS.gpg").is_file():
        raise BootError(f"no signed release in {release_dir}; run make release")

    bad_release = out / "test" / "bad-release"
    shutil.rmtree(bad_release, ignore_errors=True)
    shutil.copytree(release_dir, bad_release)
    signature = bad_release / "SHA256SUMS.gpg"
    signature.write_bytes(b"this is not a valid signature\n")

    with serve_directory(release_dir) as good_port:
        with serve_directory(bad_release) as bad_port:
            # Bad signature: the daemon must reject it and stay on slot A.
            prepare_online_update(disk, f"http://10.0.2.2:{bad_port}/", out)
            with Qemu("online-bad", out, "uefi",
                      [{"path": disk, "format": "raw"}], network=True) as qemu:
                qemu.wait(r"CARREL-TEST-UPDATE: rejected", timeout)
                qemu.wait(r"Started carrel-session\.service", timeout)
                if "CARREL-TEST-UPDATE: applying" in qemu.text():
                    raise BootError("a bad update signature was applied")
                if "carrel.slot=b" in qemu.text():
                    raise BootError("a bad update signature switched the boot slot")
                time.sleep(5)
                qemu.screenshot(out / "screenshot-online-bad-signature.png")
                qemu.powerdown()

            # Good signature: stage, ApplyUpdate through D-Bus, reboot into B.
            prepare_online_update(disk, f"http://10.0.2.2:{good_port}/", out)
            with Qemu("online-good", out, "uefi",
                      [{"path": disk, "format": "raw"}], network=True) as qemu:
                qemu.wait(r"CARREL-TEST-UPDATE: ready", timeout)
                qemu.wait(r"CARREL-TEST-UPDATE: applying", timeout)
                qemu.wait(r"carrel\.slot=b", timeout)
                qemu.wait(r"Started carrel-session\.service", timeout)
                qemu.wait(r"CARREL-BOOT: slot b marked good", timeout)
                time.sleep(5)
                qemu.screenshot(out / "screenshot-online-update.png")
                qemu.powerdown()

    version = root_version(disk, "b", out)
    if version != expect_version:
        raise BootError(f"online update booted slot B but its version is {version!r}")
    parts = read_gpt(disk)
    label = part_by_uuid(parts, ROOT_B_UUID)["name"]
    if expect_version not in label:
        raise BootError(f"online update did not relabel slot B: {label}")
    print(f"online: PASS (bad signature rejected, good update booted {expect_version})",
          flush=True)


def scenario_update(disk: Path, update_dir: Path, out: Path, timeout: float,
                    mode: str, secure: bool = False) -> None:
    label = "secureboot" if secure else mode
    overlay = qcow2_overlay(disk, out / "test" / f"update-{label}" / "disk.qcow2")
    with Qemu(f"update-{label}", out, mode,
              [{"path": overlay, "format": "qcow2"}], secure=secure) as qemu:
        qemu.wait(r"CARREL-UPDATE: applying", timeout)
        qemu.wait(r"CARREL-UPDATE: .*applied", timeout)

        # The update rebooted into slot B.  Slot B masks the session and has
        # a failing unit, so boot-complete is never reached.  After the third
        # boot the OS removes the exhausted entry and reboots into A.
        for attempt in range(1, 4):
            qemu.wait(r"carrel\.slot=b", timeout)
            print(f"update-{label}: slot B attempt {attempt}", flush=True)
            time.sleep(12)
            if attempt < 3:
                qemu.reset()
        if mode == "bios":
            # GRUB decrements to 0 on the third B attempt; the *next* boot
            # sees 0 and selects A.
            qemu.reset()

        qemu.wait(r"carrel\.slot=a", timeout)
        qemu.wait(r"Started carrel-session\.service", timeout)
        qemu.wait(r"CARREL-BOOT: slot a marked good", timeout)
        time.sleep(5)
        qemu.screenshot(out / f"screenshot-update-rollback-{label}.png")
        if secure:
            if "CARREL-BOOT: secureboot=1" not in qemu.text():
                raise BootError("the update/rollback boots did not run with Secure Boot")
            if "secureboot: Secure boot disabled" in qemu.text():
                raise BootError("Secure Boot was off during the update/rollback boots")
        qemu.powerdown()
    print(f"update-{label}: PASS", flush=True)


def flash_release(archive: Path, out: Path) -> Path:
    """Decompress a release .img.xz onto a fresh virtual external drive.

    The drive is larger than the image, like a real external drive: the image
    is written to its start with dd, as the README tells users to do.
    """
    usb = out / "test" / "release" / "usb.img"
    usb.parent.mkdir(parents=True, exist_ok=True)
    usb.unlink(missing_ok=True)
    run("truncate", "-s", "16G", usb)
    with subprocess.Popen(["xz", "-dc", str(archive)], stdout=subprocess.PIPE) as xz:
        run("dd", f"of={usb}", "bs=4M", "conv=notrunc", "status=none", stdin=xz.stdout)
        xz.stdout.close()
        if xz.wait() != 0:
            raise BootError(f"xz could not decompress {archive}")
    return usb


def scenario_release(archive: Path, out: Path, timeout: float) -> None:
    """Start Carrel from the flashed release image on UEFI, Secure Boot and BIOS."""
    for label, mode, secure in (
        ("uefi", "uefi", False), ("secureboot", "uefi", True), ("bios", "bios", False),
    ):
        usb = flash_release(archive, out)
        name = f"release-{label}"
        with Qemu(name, out, mode, [], secure=secure,
                  usb={"path": usb, "format": "raw"}) as qemu:
            wait_installer(qemu, "main menu", timeout)
            time.sleep(2)
            qemu.screenshot(out / f"screenshot-release-{label}-menu.png", light=False)
            qemu.key("ret")  # Start Carrel (the default choice)
            wait_installer(qemu, "start selected", 60)
            boot_to_editor(qemu, timeout, first_run=True)
            runtime_check(qemu)
            qemu.type_text("The quiet morning")
            time.sleep(1.5)
            qemu.screenshot(out / f"screenshot-release-{label}.png")
            # The first boot grows the data partition (carrel-grow-data) and its
            # file system (systemd-growfs) to the external drive's size.
            if "Starting systemd-growfs@data.service" not in qemu.text():
                raise BootError(f"[{name}] the data file system was not grown")
            qemu.powerdown()
        data = part_named(read_gpt(usb), "carrel-data")
        if data["size"] < 10 * 1024**3:
            raise BootError(
                f"[{name}] data is {data['size'] // 1024**2} MiB after the first boot; "
                "expected it to fill the 16 GiB external drive"
            )
        print(f"release-{label}: PASS (flashed image, Start Carrel)", flush=True)
    usb.unlink(missing_ok=True)


# Flip to True once #49 lands (Settings -> Keyboard layout applies to session).
# Can also be enabled per run via `--swedish`.
SWEDISH_CHECKS_ENABLED = False


def _ocr_expect(qemu: Qemu, path: Path, pattern: str, description: str,
                *, light: bool = True, retries: int = 4) -> str:
    """Take a screenshot and retry OCR briefly until `pattern` appears."""
    last = ""
    for _ in range(retries):
        shot = qemu.screenshot(path, light=light)
        last = ocr(shot)
        if re.search(pattern, last, re.IGNORECASE):
            return last
        time.sleep(1.0)
    raise BootError(
        f"[{qemu.name}] {description}: expected /{pattern}/i in {path.name}, "
        f"got:\n{last}"
    )


def _run_swedish_layout_checks(disk: Path, data: dict, out: Path,
                               timeout: float) -> None:
    """Verify Swedish layout (å ä ö) via physical QMP keycodes (issue #49)."""
    with mounted(disk, data, "ext4", writable=True, root=out / "test") as root:
        settings_path = root / "writing" / ".carrel" / "settings.json"
        settings = json.loads(settings_path.read_text())
        settings["keyboard"] = "se"
        settings_path.write_text(json.dumps(settings, indent=2))

    with Qemu("actions-swedish", out, "uefi",
              [{"path": disk, "format": "raw"}]) as qemu:
        boot_to_editor(qemu, timeout, first_run=False)
        qemu.key("ctrl-n")
        time.sleep(1.0)
        qemu.type_text("Swedish check\n\n")
        # Physical US keys `[`, `'`, `;` produce `å`, `ä`, `ö` on XKB `se`.
        for qcode in ("bracket_left", "apostrophe", "semicolon"):
            qemu.key(qcode)
            time.sleep(0.1)
        time.sleep(6.0)  # allow autosave
        qemu.screenshot(out / "screenshot-actions-swedish.png")
        qemu.powerdown()

    with mounted(disk, data, "ext4", root=out / "test") as root:
        texts = [
            p.read_text(errors="replace")
            for p in (root / "writing").rglob("*.md")
            if ".carrel" not in p.parts
        ]
    if not any("åäö" in text for text in texts):
        raise BootError(
            "swedish layout check failed: 'åäö' did not reach /data/writing "
            f"(files saw: {texts!r})"
        )
    print("actions (swedish): PASS (åäö typed and saved)", flush=True)


def scenario_actions(image: Path, out: Path, timeout: float,
                     *, swedish: bool = False) -> None:
    """Walk the user-facing inventory (spec/actions.md) via QMP keys.

    Checks real visible effects from screenshots/OCR and persisted state on
    /data/writing (documents, folders, and .carrel/settings.json) across a
    clean reboot. Keeps working on the US layout; pass `--swedish` (or flip
    SWEDISH_CHECKS_ENABLED) once #49 lands.
    """
    base = out / "test" / "actions"
    base.mkdir(parents=True, exist_ok=True)
    disk = base / "disk.img"
    disk.unlink(missing_ok=True)
    run("cp", "--reflink=auto", "--sparse=always", image, disk)

    shots = base / "shots"
    shots.mkdir(parents=True, exist_ok=True)

    with Qemu("actions", out, "uefi", [{"path": disk, "format": "raw"}]) as qemu:
        # 1. First run -> editor.
        boot_to_editor(qemu, timeout, first_run=True)
        runtime_check(qemu)

        # 2. Editor typing (auto-named from first line) + F1 bottom line toggle.
        qemu.type_text("First draft\n\nAlpha line.")
        time.sleep(1.5)
        qemu.key("f1")
        time.sleep(0.6)
        qemu.key("f1")
        time.sleep(0.6)

        # 3. New file (Ctrl+N), typing, and Rename current file (Ctrl+Shift+R).
        qemu.key("ctrl-n")
        time.sleep(1.2)
        qemu.type_text("Second note\n\nBody text. ")
        time.sleep(1.0)
        qemu.key("ctrl-shift-r")
        time.sleep(0.8)
        _ocr_expect(qemu, shots / "rename-sheet.png", r"Rename|Save",
                    "Ctrl+Shift+R did not open Rename sheet")
        qemu.type_text("Renamed Note")
        qemu.key("ret")
        time.sleep(1.2)

        # 4. Key sheet (Ctrl+/) -> verify shortcuts card -> Esc closes.
        qemu.key("ctrl-slash")
        time.sleep(0.8)
        _ocr_expect(qemu, shots / "key-sheet.png",
                    r"Keyboard shortcuts|Writing",
                    "Ctrl+/ did not open Keyboard shortcuts card")
        qemu.key("esc")
        time.sleep(0.6)

        # 5. Command palette (Ctrl+K): Font Mono, Theme Sepia, Typewriter mode.
        for query, label in (
            ("mono", "Font: Mono"),
            ("sepia", "Theme: Sepia"),
            ("typewriter", "Turn typewriter mode on"),
        ):
            qemu.key("ctrl-k")
            time.sleep(0.7)
            if query == "mono":
                _ocr_expect(qemu, shots / "palette.png",
                            r"New file|Settings|Export",
                            "Ctrl+K did not open Command palette")
            qemu.type_text(query)
            time.sleep(0.5)
            qemu.key("ret")
            time.sleep(0.8)

        # 6. Verify Typewriter mode actually blocks Backspace in the editor.
        qemu.type_text("KEEP")
        time.sleep(0.4)
        qemu.key("backspace")
        time.sleep(0.2)
        qemu.key("backspace")
        time.sleep(0.4)

        # 7. Quotes palette (Ctrl+') -> opens card -> Esc removes inserted `@`.
        qemu.key("ctrl-apostrophe")
        time.sleep(0.8)
        _ocr_expect(qemu, shots / "quotes.png",
                    r"Sync once|Nostos|Quote|Close",
                    "Ctrl+' did not open Quotes palette")
        qemu.key("esc")
        time.sleep(0.6)

        # 8. Files screen (Ctrl+O) + New folder (Ctrl+Shift+N).
        qemu.key("ctrl-o")
        time.sleep(1.0)
        _ocr_expect(qemu, shots / "files.png", r"Files",
                    "Ctrl+O did not open Files")
        qemu.key("ctrl-shift-n")
        time.sleep(0.8)
        qemu.type_text("Drafts")
        qemu.key("ret")
        time.sleep(1.0)
        qemu.key("esc")
        time.sleep(0.6)

        # 9. History screen (Ctrl+H).
        qemu.key("ctrl-h")
        time.sleep(1.0)
        _ocr_expect(qemu, shots / "history.png", r"History",
                    "Ctrl+H did not open History")
        qemu.key("esc")
        time.sleep(0.6)

        # 10. Export & Import screen (Ctrl+E, I, E).
        qemu.key("ctrl-e")
        time.sleep(1.0)
        _ocr_expect(qemu, shots / "export.png", r"Export",
                    "Ctrl+E did not open Export")
        qemu.key("i")
        time.sleep(0.8)
        _ocr_expect(qemu, shots / "import.png", r"Import",
                    "I did not switch Export to Import")
        qemu.key("e")
        time.sleep(0.6)
        qemu.key("esc")
        time.sleep(0.6)

        # 11. Sync screen (Ctrl+Shift+S).
        qemu.key("ctrl-shift-s")
        time.sleep(1.0)
        _ocr_expect(qemu, shots / "sync.png", r"Sync|Nostos|Not connected",
                    "Ctrl+Shift+S did not open Sync")
        qemu.key("esc")
        time.sleep(0.6)

        # 12. Settings screen (Ctrl+,): change Size, Daily goal, Goal progress,
        # Focus mode, and Clock, then jump directly to Files via Ctrl+O.
        # Under Theme=Sepia (non-Automatic, no update ready), row order is:
        # 0 Theme, 1 Font, 2 Size, 3 Daily goal, 4 Goal progress, 5 Focus mode,
        # 6 Typewriter mode, 7 Spellcheck, 8 Clock.
        qemu.key("ctrl-comma")
        time.sleep(1.0)
        _ocr_expect(qemu, shots / "settings.png", r"Settings",
                    "Ctrl+, did not open Settings")
        qemu.key("home")
        time.sleep(0.3)
        for steps in (2, 1, 1, 1, 3):
            for _ in range(steps):
                qemu.key("down")
                time.sleep(0.25)
            qemu.key("right")
            time.sleep(0.4)
        # Global Ctrl+O from Settings opens Files directly.
        qemu.key("ctrl-o")
        time.sleep(0.8)
        _ocr_expect(qemu, shots / "files-from-settings.png", r"Files",
                    "Ctrl+O from Settings did not open Files")
        qemu.key("esc")
        time.sleep(6.0)  # allow idle autosave before clean shutdown
        qemu.screenshot(out / "screenshot-actions-editor.png")
        qemu.powerdown()

    # Check every persisted effect on /data/writing.
    parts = read_gpt(disk)
    data = part_named(parts, "carrel-data")
    failures: list[str] = []
    with mounted(disk, data, "ext4", root=out / "test") as root:
        writing = root / "writing"
        settings_path = writing / ".carrel" / "settings.json"
        expect(settings_path.is_file(),
               "settings.json missing on /data/writing/.carrel", failures)
        if settings_path.is_file():
            settings = json.loads(settings_path.read_text())
            expect(settings.get("first_run_complete") is True,
                   f"first_run_complete not True: {settings!r}", failures)
            expect(settings.get("theme") == "sepia",
                   f"theme expected 'sepia', got {settings.get('theme')!r}",
                   failures)
            expect(settings.get("font") == "mono",
                   f"font expected 'mono', got {settings.get('font')!r}",
                   failures)
            expect(settings.get("font_size") == 20,
                   f"font_size expected 20, got {settings.get('font_size')!r}",
                   failures)
            expect(settings.get("daily_goal") == 600,
                   f"daily_goal expected 600, got {settings.get('daily_goal')!r}",
                   failures)
            expect(settings.get("goal_display") == "hairline",
                   f"goal_display expected 'hairline', got {settings.get('goal_display')!r}",
                   failures)
            expect(settings.get("focus_mode") is True,
                   f"focus_mode expected True, got {settings.get('focus_mode')!r}",
                   failures)
            expect(settings.get("typewriter_mode") is True,
                   f"typewriter_mode expected True, got {settings.get('typewriter_mode')!r}",
                   failures)
            expect(settings.get("clock") is True,
                   f"clock expected True, got {settings.get('clock')!r}",
                   failures)

        expect((writing / "Drafts").is_dir(),
               "Ctrl+Shift+N did not create /data/writing/Drafts", failures)
        renamed = writing / "Renamed Note.md"
        expect(renamed.is_file(),
               "Ctrl+Shift+R did not create /data/writing/Renamed Note.md",
               failures)
        if renamed.is_file():
            body = renamed.read_text(errors="replace")
            expect("Body text." in body,
                   f"Renamed Note.md missing typed text: {body!r}", failures)
            expect("KEEP" in body,
                   f"Typewriter mode did not block Backspace (KEEP lost): {body!r}",
                   failures)
            expect("@" not in body,
                   f"Esc from Ctrl+' Quotes palette left stray '@': {body!r}",
                   failures)
        first_doc = writing / "First draft.md"
        expect(first_doc.is_file(),
               "First document was not auto-named 'First draft.md'", failures)
        if first_doc.is_file():
            expect("Alpha line." in first_doc.read_text(errors="replace"),
                   "First draft.md missing typed text", failures)

    if failures:
        raise BootError("actions disk checks failed: " + "; ".join(failures))

    # Boot 2: verify settings and documents survive a clean reboot without
    # returning to First run.
    with Qemu("actions-reboot", out, "uefi",
              [{"path": disk, "format": "raw"}]) as qemu:
        boot_to_editor(qemu, timeout, first_run=False)
        shot = qemu.screenshot(out / "screenshot-actions-after-reboot.png")
        text = ocr(shot)
        if re.search(r"Welcome to Carrel|Connect sync", text, re.IGNORECASE):
            raise BootError(f"First run reappeared after reboot:\n{text}")
        qemu.powerdown()

    print("actions: PASS (shortcuts, palette, settings, and persistence verified)",
          flush=True)

    if swedish or SWEDISH_CHECKS_ENABLED:
        _run_swedish_layout_checks(disk, data, out, timeout)


# --------------------------------------------------------------------------
# Entry point


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--update-dir", type=Path,
                        help="directory with the offline update fixture")
    parser.add_argument("--release-dir", type=Path,
                        help="directory with the signed online release")
    parser.add_argument("--release-image", type=Path,
                        help="release .img.xz for the 'release' scenario")
    parser.add_argument("--scenario", default="all",
                        choices=("all", "checks", "uefi", "bios", "keyboard",
                                 "install", "recovery", "powercut", "shutdown-sync",
                                 "palette-power", "typing-slow-disk", "update",
                                 "online", "secureboot",
                                 "update-secureboot", "release", "actions"))
    parser.add_argument("--swedish", action="store_true",
                        help="enable Swedish layout typing checks in the 'actions' scenario")
    parser.add_argument("--timeout", type=float, default=300.0,
                        help="seconds for one boot (default 300)")
    parser.add_argument("--expect-version", default="0.3.0",
                        help="version the online update must boot (default 0.3.0)")
    args = parser.parse_args()

    args.image = args.image.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    if not args.image.exists():
        print(f"image not found: {args.image}", file=sys.stderr)
        return 2
    if args.update_dir is None:
        args.update_dir = args.image.parent / "update"
    if args.release_dir is None:
        args.release_dir = args.image.parent.parent / "release-test"
    args.update_dir = args.update_dir.resolve()
    args.release_dir = args.release_dir.resolve()

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
    installed_sb: Path | None = None
    try:
        if args.scenario in ("all", "uefi"):
            scenario_boot("uefi", args.image, args.out, args.timeout)
        if args.scenario in ("all", "bios"):
            scenario_boot("bios", args.image, args.out, args.timeout)
        if args.scenario in ("all", "keyboard"):
            scenario_keyboard(args.image, args.out, args.timeout)
        if args.scenario in ("all", "actions"):
            scenario_actions(args.image, args.out, args.timeout,
                             swedish=args.swedish)
        if args.scenario in ("all", "shutdown-sync"):
            scenario_shutdown_sync(args.image, args.out, args.timeout)
        if args.scenario in ("all", "palette-power"):
            scenario_palette_power(args.image, args.out, args.timeout)
        if args.scenario in ("all", "typing-slow-disk"):
            scenario_typing_slow_disk(args.image, args.out, args.timeout)
        if args.scenario in ("all", "install", "recovery", "powercut", "update",
                             "online"):
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
            stage_update(installed, args.update_dir, args.out)
            scenario_update(installed, args.update_dir, args.out, args.timeout, "uefi")
            scenario_update(installed, args.update_dir, args.out, args.timeout, "bios")
            clear_update(installed, args.out)
        if args.scenario in ("all", "online"):
            scenario_online_update(installed, args.release_dir, args.out, args.timeout,
                                   args.expect_version)
        # Secure Boot: OVMF with the Microsoft keys, enforcing.
        if args.scenario in ("all", "secureboot"):
            scenario_boot("uefi", args.image, args.out, args.timeout, secure=True)
            installed_sb = scenario_install(args.image, args.out, args.timeout,
                                            secure=True)
        if args.scenario in ("all", "update-secureboot"):
            if installed_sb is None:
                reused = args.out / "test" / "install-secureboot" / "disk.img"
                if reused.exists() and args.scenario != "all":
                    installed_sb = reused
                    print(f"reusing installed disk {installed_sb}", flush=True)
                else:
                    installed_sb = scenario_install(args.image, args.out,
                                                    args.timeout, secure=True)
            stage_update(installed_sb, args.update_dir, args.out)
            scenario_update(installed_sb, args.update_dir, args.out, args.timeout,
                            "uefi", secure=True)
            clear_update(installed_sb, args.out)
        if args.scenario == "release":
            if args.release_image is None or not args.release_image.is_file():
                raise BootError("--release-image must name the release .img.xz")
            scenario_release(args.release_image.resolve(), args.out, args.timeout)
    except BootError as exc:
        errors.append(str(exc))
        print(f"FAILED: {exc}", file=sys.stderr)

    if errors:
        return 1
    print(f"all scenarios passed; screenshots in {args.out}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
