#!/usr/bin/env python3
"""Boot the Stylos image in QEMU and check the WP1 gate.

Boots the same disk image twice: once through OVMF (UEFI + systemd-boot) and
once through SeaBIOS (legacy BIOS + GRUB).

Assertions have two parts:

1. Offline image checks (GPT layout, service/mask symlinks, binaries) run
   against the root filesystem before booting.
2. Runtime checks read the guest serial console: systemd reports every unit
   start/failure there, and the session must reach graphical.target without
   restarting.  A screenshot of the Cage output is written to
   out/screenshot-<mode>.png and must contain the light Stylos window rather
   than a black framebuffer.

Only the Python standard library and the tools in dev/Dockerfile are used.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import socket
import struct
import subprocess
import sys
import time
import uuid
from pathlib import Path

OVMF_CODE = "/usr/share/OVMF/OVMF_CODE_4M.fd"
OVMF_VARS = "/usr/share/OVMF/OVMF_VARS_4M.fd"

# GPT partition type GUIDs.
ESP_GUID = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
BIOS_BOOT_GUID = "21686148-6449-6e6f-744e-656564454649"
ROOT_GUIDS = {
    "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",  # root x86-64
    "0fc63daf-8483-4772-8e79-3d69d8477de4",  # Linux filesystem data
}


class BootError(RuntimeError):
    pass


ANSI_RE = re.compile(r"\x1b\[[0-9;?!]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Za-z0-9]")


def clean_serial(text: str) -> str:
    """Strip the ANSI escapes systemd writes to the console."""
    return ANSI_RE.sub("", text)


class QMP:
    """Minimal QMP client (one command in flight at a time)."""

    def __init__(self, path: Path, timeout: float = 30.0) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(5.0)
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
        try:
            self.reader.close()
        except Exception:
            pass
        try:
            self.sock.close()
        except Exception:
            pass


def read_gpt(image: Path) -> list[dict]:
    """Return the GPT partition table as a list of dicts."""
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
                    "name": entry[56:128].decode("utf-16le").rstrip("\0"),
                }
            )
    return entries


def find_root(image: Path) -> tuple[int, int]:
    for part in read_gpt(image):
        if part["type"] in ROOT_GUIDS:
            return part["first"] * 512, (part["last"] - part["first"] + 1) * 512
    raise BootError("no Linux root partition in the image")


class LoopDevice:
    """Attach the root partition so debugfs can read it."""

    def __init__(self, image: Path, offset: int, size: int) -> None:
        self.image = image
        self.offset = offset
        self.size = size
        self.device: str | None = None

    def __enter__(self) -> str:
        self.device = subprocess.check_output(
            [
                "losetup", "-f", "--show",
                "--offset", str(self.offset),
                "--sizelimit", str(self.size),
                str(self.image),
            ],
            text=True,
        ).strip()
        return self.device

    def __exit__(self, *exc: object) -> None:
        if self.device:
            subprocess.run(["losetup", "-d", self.device], check=False)


def debugfs(device: str, command: str) -> str:
    result = subprocess.run(
        ["debugfs", "-R", command, device],
        capture_output=True, text=True,
    )
    return result.stdout + result.stderr


def image_file(device: str, path: str) -> str:
    return debugfs(device, f"cat {path}")


def image_stat(device: str, path: str) -> str:
    return debugfs(device, f"stat {path}")


def assert_image(image: Path) -> None:
    parts = read_gpt(image)
    types = {p["type"] for p in parts}
    if ESP_GUID not in types:
        raise BootError("image has no EFI system partition")
    if BIOS_BOOT_GUID not in types:
        raise BootError("image has no BIOS boot partition")

    offset, size = find_root(image)
    with LoopDevice(image, offset, size) as dev:
        failures: list[str] = []

        def expect(condition: bool, message: str) -> None:
            if not condition:
                failures.append(message)

        for binary in (
            "/usr/bin/cage",
            "/usr/bin/stylos-shell",
            "/usr/bin/pandoc",
            "/usr/bin/typst",
            "/usr/sbin/NetworkManager",
            "/usr/sbin/rfkill",
        ):
            stat = image_stat(dev, binary)
            expect("File not found" not in stat, f"missing {binary}")
            match = re.search(r"Size: (\d+)", stat)
            expect(match is not None and int(match.group(1)) > 0,
                   f"empty {binary}")

        # Cage must be the one and only session client.
        session = image_file(dev, "/etc/systemd/system/stylos-session.service")
        exec_line = next(
            (line for line in session.splitlines()
             if line.startswith("ExecStart=")), ""
        )
        expect(
            re.fullmatch(
                r"ExecStart=/usr/bin/cage(?: -[a-z]+)* -- /usr/bin/stylos-shell",
                exec_line.strip(),
            ) is not None,
            f"session does not start cage with stylos-shell: {exec_line!r}",
        )
        # cage -s would allow VT switching (rule 1).
        expect(" -s" not in exec_line, "session passes cage -s (VT switching)")
        expect("Restart=always" in session, "session does not restart")
        expect("User=writer" in session, "session does not run as writer")

        # No getty may run on any VT: every template is masked and the default
        # tty1 instance is not enabled.
        for unit in (
            "getty@.service",
            "serial-getty@.service",
            "console-getty.service",
            "autovt@.service",
        ):
            stat = image_stat(dev, f"/etc/systemd/system/{unit}")
            expect('Fast link dest: "/dev/null"' in stat or
                   'Fast link dest: /dev/null' in stat,
                   f"{unit} is not masked")
        getty_link = image_stat(
            dev, "/etc/systemd/system/getty.target.wants/getty@tty1.service"
        )
        expect("File not found" in getty_link,
               "getty@tty1 is still enabled")

        # Enabled services.
        for link, target in (
            ("/etc/systemd/system/graphical.target.wants/stylos-session.service",
             "stylos-session.service"),
            ("/etc/systemd/system/multi-user.target.wants/stylos-rfkill.service",
             "stylos-rfkill.service"),
            ("/etc/systemd/system/multi-user.target.wants/NetworkManager.service",
             "NetworkManager.service"),
        ):
            stat = image_stat(dev, link)
            expect(target in stat, f"{link} is missing or wrong")
            expect("Type: symlink" in stat, f"{link} is not a symlink")

        # The writer account and its data directory.
        data = image_stat(dev, "/data/writing")
        expect(re.search(r"User:\s+1000\b", data) is not None,
               "/data/writing is not owned by writer")
        passwd = image_file(dev, "/etc/passwd")
        expect("writer:x:1000:1000:" in passwd, "writer user is missing")

        # Product defaults that are easy to lose in a refactor.
        for path, needle in (
            ("/etc/systemd/logind.conf.d/50-stylos.conf",
             "HandleLidSwitch=suspend"),
            ("/etc/sysctl.d/50-stylos.conf", "kernel.sysrq = 0"),
            ("/etc/NetworkManager/conf.d/10-stylos.conf", "no-auto-default=*"),
        ):
            content = image_file(dev, path)
            expect(needle in content, f"{path} does not contain {needle!r}")

        # systemd-networkd and timesyncd would run next to NetworkManager and
        # talk to the network in the background.
        for unit in (
            "systemd-networkd.service",
            "systemd-networkd.socket",
            "systemd-networkd-wait-online.service",
            "systemd-timesyncd.service",
        ):
            stat = image_stat(dev, f"/etc/systemd/system/{unit}")
            expect('Fast link dest: "/dev/null"' in stat or
                   "Fast link dest: /dev/null" in stat,
                   f"{unit} is not masked")

        # No terminal emulator may ship in the image.
        for binary in (
            "/usr/bin/foot", "/usr/bin/xterm", "/usr/bin/alacritty",
            "/usr/bin/kitty", "/usr/bin/gnome-terminal", "/usr/bin/konsole",
            "/usr/bin/weston-terminal",
        ):
            stat = image_stat(dev, binary)
            expect("File not found" in stat,
                   f"terminal emulator installed: {binary}")

        if failures:
            raise BootError("; ".join(failures))


def wait_for_serial(serial_log: Path, patterns: list[str], timeout: float) -> str:
    deadline = time.monotonic() + timeout
    while True:
        try:
            text = clean_serial(serial_log.read_text(errors="replace"))
        except FileNotFoundError:
            text = ""
        if all(re.search(pattern, text) for pattern in patterns):
            return text
        if time.monotonic() > deadline:
            missing = [p for p in patterns if not re.search(p, text)]
            raise BootError(f"timed out waiting for serial output: {missing}")
        time.sleep(0.5)


def check_runtime(serial_log: Path, mode: str) -> None:
    text = clean_serial(serial_log.read_text(errors="replace"))
    failures = []

    # Prove which firmware path actually booted: systemd-boot hands over to
    # the EFI stub, while GRUB passes a legacy BOOT_IMAGE= command line.
    if mode == "uefi":
        if "EFI stub" not in text:
            failures.append("UEFI boot did not use the EFI stub / systemd-boot")
    else:
        if "BOOT_IMAGE=" not in text:
            failures.append("BIOS boot did not use GRUB")
        if "EFI stub" in text:
            failures.append("BIOS boot unexpectedly used the EFI stub")

    if re.search(r"getty@tty[1-6]\.service", text):
        failures.append("a getty started on tty1-6")
    if re.search(r"serial-getty@", text):
        failures.append("a serial getty started")
    if "Started systemd-networkd.service" in text:
        failures.append("systemd-networkd started")
    if "Started systemd-timesyncd.service" in text:
        failures.append("systemd-timesyncd started")
    if "Failed to start stylos-session.service" in text:
        failures.append("stylos-session failed to start")
    if re.search(r"stylos-session\.service.*(Main process exited|Failed with result)",
                 text):
        failures.append("stylos-session exited after starting")
    # Paired with a screenshot of the Cage output, a clean single start means
    # the compositor and client are still up.
    starts = len(re.findall(r"Started stylos-session\.service", text))
    if starts != 1:
        failures.append(f"stylos-session started {starts} times")
    if "Kernel panic" in text:
        failures.append("kernel panic")
    if "Reached target graphical.target" not in text:
        failures.append("graphical.target was not reached")
    if "Started NetworkManager.service" not in text:
        failures.append("NetworkManager did not start")
    if "Finished stylos-rfkill.service" not in text:
        failures.append("stylos-rfkill did not run")

    if failures:
        raise BootError("; ".join(failures))


def take_screenshot(qmp: QMP, out: Path) -> Path:
    png = out / "screenshot.png"
    try:
        qmp.command("screendump", filename=str(png), format="png")
    except BootError:
        # Older QEMU only writes PPM; ImageMagick is in the dev container.
        ppm = out / "screenshot.ppm"
        qmp.command("screendump", filename=str(ppm))
        subprocess.run(["convert", str(ppm), str(png)], check=True)
        ppm.unlink(missing_ok=True)

    if not png.exists() or png.stat().st_size < 1024:
        raise BootError(f"screenshot looks empty: {png}")
    if shutil.which("identify"):
        mean = subprocess.run(
            ["identify", "-format", "%[fx:mean]", str(png)],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        # Cage shows the cream Stylos window; a text console is nearly black.
        if float(mean) < 0.5:
            raise BootError(
                f"screenshot is too dark (mean={mean}); Cage window missing?"
            )
    return png


def run_mode(mode: str, image: Path, out: Path, timeout: float) -> None:
    mode_dir = out / f"boot-{mode}"
    mode_dir.mkdir(parents=True, exist_ok=True)
    qmp_sock = mode_dir / "qmp.sock"
    serial_log = out / f"serial-{mode}.log"
    qemu_log = out / f"qemu-{mode}.log"
    qmp_sock.unlink(missing_ok=True)
    serial_log.unlink(missing_ok=True)

    cmd = [
        "qemu-system-x86_64",
        "-name", f"stylos-{mode}",
        "-machine", "q35,accel=kvm:tcg",
        "-m", "2048",
        "-smp", "2",
        "-display", "none",
        "-vga", "std",
        "-boot", "order=c,menu=off",
        "-drive", f"file={image},format=raw,if=virtio,cache=writeback",
        "-device", "virtio-rng-pci",
        "-nic", "none",
        "-serial", f"file:{serial_log}",
        "-monitor", "none",
        "-qmp", f"unix:{qmp_sock},server=on,wait=off",
    ]

    vars_copy = mode_dir / "OVMF_VARS.fd"
    if mode == "uefi":
        if not Path(OVMF_CODE).exists() or not Path(OVMF_VARS).exists():
            raise BootError("OVMF firmware is not installed in the dev container")
        shutil.copyfile(OVMF_VARS, vars_copy)
        cmd[cmd.index("-drive"):cmd.index("-drive")] = [
            "-drive", f"if=pflash,format=raw,readonly=on,file={OVMF_CODE}",
            "-drive", f"if=pflash,format=raw,file={vars_copy}",
        ]

    print(f"[{mode}] starting QEMU", flush=True)
    with qemu_log.open("w") as log:
        proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT)

    qmp: QMP | None = None
    try:
        wait_for_serial(
            serial_log,
            [
                r"Started stylos-session\.service",
                r"Reached target graphical\.target",
            ],
            timeout,
        )
        # Give GTK a moment to draw its first frame, then make sure the
        # session did not crash in the meantime.
        time.sleep(5)
        check_runtime(serial_log, mode)

        qmp = QMP(qmp_sock, timeout=30)
        png = take_screenshot(qmp, out)
        final = out / f"screenshot-{mode}.png"
        png.replace(final)
        print(f"[{mode}] PASS: {final}", flush=True)
    except BootError as exc:
        print(f"[{mode}] FAIL: {exc}", flush=True)
        print(f"[{mode}] serial tail:\n{serial_tail(serial_log)}",
              file=sys.stderr, flush=True)
        raise
    finally:
        if qmp is not None:
            try:
                qmp.command("quit")
            except Exception:
                proc.terminate()
            qmp.close()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        qmp_sock.unlink(missing_ok=True)
        vars_copy.unlink(missing_ok=True)
        try:
            mode_dir.rmdir()
        except OSError:
            pass


def serial_tail(serial_log: Path, lines: int = 80) -> str:
    try:
        text = serial_log.read_text(errors="replace").splitlines()
    except FileNotFoundError:
        return "(no serial log)"
    return "\n".join(text[-lines:])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--mode", choices=("uefi", "bios", "both"), default="both")
    parser.add_argument("--timeout", type=float, default=240.0,
                        help="seconds to wait for boot and session (default 240)")
    args = parser.parse_args()

    args.image = args.image.resolve()
    args.out.mkdir(parents=True, exist_ok=True)
    if not args.image.exists():
        print(f"image not found: {args.image}", file=sys.stderr)
        return 2

    try:
        assert_image(args.image)
        print("offline image checks passed", flush=True)
    except BootError as exc:
        print(f"FAILED: {exc}", file=sys.stderr)
        return 1

    modes = ("uefi", "bios") if args.mode == "both" else (args.mode,)
    errors: list[str] = []
    for mode in modes:
        try:
            run_mode(mode, args.image, args.out, args.timeout)
        except BootError as exc:
            errors.append(f"{mode}: {exc}")

    if errors:
        print("FAILED: " + "; ".join(errors), file=sys.stderr)
        return 1
    print(f"all modes passed; screenshots in {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
