#!/usr/bin/env python3
"""Finish the Stylos disk image after mkosi has assembled it.

mkosi builds one unified kernel image with its own command line.  Stylos needs
three (slot A, slot B and the recovery menu), each with a different root=, and
the BIOS GRUB menu needs the same slots.  Doing this in a post-output script
keeps the slot-specific values next to the partition definitions instead of
spreading them through mkosi profiles.

The script also writes the offline update fixture used by tests/boot/:
a copy of the root filesystem and a UKI for slot B whose command line masks
the session, so boot never reaches boot-complete.target.
"""

from __future__ import annotations

import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import uuid
from pathlib import Path

ESP_GUID = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
ROOT_A_UUID = "11111111-1111-4111-8111-111111111111"
ROOT_B_UUID = "22222222-2222-4222-8222-222222222222"
DATA_UUID = "33333333-3333-4333-8333-333333333333"

BASE_CMDLINE = (
    "console=tty0 console=ttyS0,115200 systemd.getty_auto=no "
    "ro rootfstype=erofs"
)
VERSION = os.environ.get("STYLOS_VERSION", "0.1.0")
UPDATE_VERSION = os.environ.get("STYLOS_UPDATE_VERSION", "0.2.0")

UKIFY = "/usr/bin/ukify"
STUB = "/usr/lib/systemd/boot/efi/linuxx64.efi.stub"


def run(*args: object, **kwargs: object) -> subprocess.CompletedProcess:
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def read_gpt(image: Path) -> list[dict]:
    with image.open("rb") as f:
        f.seek(512)
        header = f.read(92)
        if header[:8] != b"EFI PART":
            raise SystemExit(f"{image} has no GPT header")
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


class Esp:
    """Small mtools wrapper for the FAT partition inside the image."""

    def __init__(self, image: Path, offset: int) -> None:
        self.spec = f"{image}@@{offset}"

    def mdir(self, path: str) -> str:
        return subprocess.run(
            ["mdir", "-i", self.spec, "-b", f"::{path}"],
            check=True, capture_output=True, text=True,
        ).stdout

    def copy_in(self, source: Path, dest: str) -> None:
        run("mcopy", "-o", "-i", self.spec, source, f"::{dest}")

    def copy_out(self, source: str, dest: Path) -> None:
        run("mcopy", "-o", "-i", self.spec, f"::{source}", dest)

    def remove(self, path: str) -> None:
        subprocess.run(
            ["mdel", "-i", self.spec, f"::{path}"],
            check=False, capture_output=True,
        )

    def write(self, text: str, dest: str, work: Path, name: str) -> None:
        path = work / name
        path.write_text(text)
        self.copy_in(path, dest)


def kernel_versions(esp: Esp) -> str:
    listing = esp.mdir("/stylos")
    for line in listing.splitlines():
        if not line.endswith("/"):
            continue
        name = line.rstrip("/").split("/")[-1].strip()
        if not name:
            continue
        if re.fullmatch(r"[0-9][0-9A-Za-z.+~_-]*", name):
            return name
    raise SystemExit("no kernel version directory on the ESP")


def extract_sections(esp: Esp, work: Path, kver: str) -> tuple[Path, Path, Path, Path]:
    esp.copy_out(f"/stylos/{kver}/vmlinuz", work / "vmlinuz")
    esp.copy_out("/stylos/initrd", work / "initrd")
    esp.copy_out(f"/stylos/{kver}/kernel-modules.initrd", work / "modules.initrd")

    base = work / "base.efi"
    for line in esp.mdir("/EFI/Linux").splitlines():
        name = line.rstrip("/").split("/")[-1]
        if name.endswith(".efi") and name.startswith("stylos-"):
            esp.copy_out(f"/EFI/Linux/{name}", base)
            break
    if not base.exists():
        raise SystemExit("mkosi did not produce a UKI on the ESP")

    # objcopy insists on an output file even when it only dumps a section.
    # Give it a throwaway copy instead of /dev/null (some binutils versions
    # treat that as a truncated output and fail).
    copied = work / "base-with-osrel.efi"
    run("objcopy", "--dump-section", f".osrel={work / 'osrel'}", base, copied)
    copied.unlink(missing_ok=True)
    return work / "vmlinuz", work / "initrd", work / "modules.initrd", work / "osrel"


def build_uki(
    work: Path, output: Path, kver: str, cmdline: str,
) -> None:
    run(
        UKIFY, "build",
        f"--linux={work / 'vmlinuz'}",
        f"--initrd={work / 'initrd'}",
        f"--initrd={work / 'modules.initrd'}",
        f"--os-release=@{work / 'osrel'}",
        f"--uname={kver}",
        f"--cmdline={cmdline}",
        f"--stub={STUB}",
        f"--output={output}",
        stdout=subprocess.DEVNULL,
    )


def cmdline(slot: str, extra: str = "") -> str:
    uuid = ROOT_A_UUID if slot == "a" else ROOT_B_UUID
    line = f"root=PARTUUID={uuid} {BASE_CMDLINE} stylos.slot={slot}"
    return f"{line} {extra}".strip()


def grub_cfg(kver: str) -> str:
    def entry(ident: str, title: str, slot: str, extra: str = "") -> str:
        uuid = ROOT_A_UUID if slot == "a" else ROOT_B_UUID
        cmd = f"root=PARTUUID={uuid} {BASE_CMDLINE} stylos.slot={slot} {extra}".strip()
        return (
            f'menuentry "{title}" --id {ident} {{\n'
            f"    linux /stylos/{kver}/vmlinuz {cmd}\n"
            f"    initrd /stylos/initrd /stylos/{kver}/kernel-modules.initrd\n"
            "}\n"
        )

    return (
        "set timeout=2\n"
        "if [ -f /grub/grubenv ]; then\n"
        "    load_env\n"
        "fi\n"
        # GRUB has no EFI boot counter.  The update service sets
        # stylos_default=<slot> and stylos_<slot>_tries=3; each attempt
        # decrements the counter and after three failed boots the other slot
        # becomes the default.  boot-complete (BIOS path) resets the counter.
        "if [ -z \"${stylos_b_tries}\" ]; then set stylos_b_tries=0; fi\n"
        "if [ \"${stylos_default}\" = \"b\" ]; then\n"
        "    if [ \"${stylos_b_tries}\" -gt 0 ]; then\n"
        "        set stylos_b_tries=$((${stylos_b_tries} - 1))\n"
        "        save_env stylos_b_tries\n"
        "        set default=stylos-b\n"
        "    else\n"
        "        set default=stylos-a\n"
        "        set stylos_default=a\n"
        "    fi\n"
        "else\n"
        "    set default=stylos-a\n"
        "fi\n"
        "\n"
        + entry("stylos-a", "Stylos", "a")
        + "\n"
        + entry("stylos-b", "Stylos (previous system)", "b")
        + "\n"
        + entry(
            "stylos-recovery", "Stylos recovery", "a",
            "systemd.unit=stylos-recovery.target",
        )
    )


def build_update_payload(
    image: Path, root: dict, work: Path, out: Path, kver: str,
) -> None:
    payload = out / "update" / "payload"
    payload.mkdir(parents=True, exist_ok=True)

    # Build a deliberately broken copy of the root filesystem: it is slot A
    # plus a unit that fails, so the new slot can boot but never reaches
    # boot-complete.target.  Extracting and repacking also keeps the payload
    # near 700 MiB instead of the 2 GiB partition size.
    bad_root = work / "bad-root"
    if bad_root.exists():
        shutil.rmtree(bad_root)
    run(
        "fsck.erofs", f"--offset={root['first'] * 512}",
        f"--extract={bad_root}", "--overwrite", "--preserve", image,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    unit = bad_root / "usr/lib/systemd/system/stylos-broken.service"
    unit.parent.mkdir(parents=True, exist_ok=True)
    unit.write_text(
        "[Unit]\nDescription=Broken update fixture\n\n"
        "[Service]\nType=oneshot\nExecStart=/bin/false\n\n"
        "[Install]\nWantedBy=multi-user.target\n"
    )
    wants = bad_root / "etc/systemd/system/multi-user.target.wants"
    wants.mkdir(parents=True, exist_ok=True)
    link = wants / "stylos-broken.service"
    link.unlink(missing_ok=True)
    link.symlink_to("/usr/lib/systemd/system/stylos-broken.service")

    run(
        "mkfs.erofs", "-zlz4hc,9", "-L", "stylos-root-b",
        payload / f"stylos-root-{UPDATE_VERSION}.raw", bad_root,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )

    broken = cmdline("b", "systemd.mask=stylos-session.service")
    build_uki(work, payload / f"stylos-b-{UPDATE_VERSION}.efi", kver, broken)

    (out / "update" / "README").write_text(
        f"""Stylos {UPDATE_VERSION} offline update fixture (built by image/assemble.py).

Copy payload/ to /data/.stylos/update/payload/ on a running Stylos and
reboot.  stylos-update.service applies the inactive slot with
systemd-sysupdate, sets the boot-counting entry as the default and reboots.

This particular payload is intentionally broken: its kernel command line
masks stylos-session.service, so boot-complete.target is never reached and
the boot counter falls back to the previous slot after three attempts.
"""
    )


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: assemble.py IMAGE OUTDIR", file=sys.stderr)
        return 2
    image = Path(sys.argv[1])
    out = Path(sys.argv[2])
    if not image.exists():
        print(f"image missing: {image}", file=sys.stderr)
        return 2
    if not Path(UKIFY).exists() or not Path(STUB).exists():
        print("ukify or the systemd-stub is missing from the build host", file=sys.stderr)
        return 2

    parts = read_gpt(image)
    esp_part = next(p for p in parts if p["type"] == ESP_GUID)
    roots = sorted(
        (p for p in parts if p["name"].startswith("stylos-root-")),
        key=lambda p: p["first"],
    )
    if len(roots) != 2:
        raise SystemExit(f"expected two root slots, found {len(roots)}")
    root_a = roots[0]
    esp = Esp(image, esp_part["first"] * 512)
    kver = kernel_versions(esp)

    with tempfile.TemporaryDirectory(dir=out) as tmp:
        work = Path(tmp)
        extract_sections(esp, work, kver)

        uki_a = work / "stylos-a.efi"
        uki_recovery = work / "stylos-recovery.efi"
        build_uki(work, uki_a, kver, cmdline("a"))
        build_uki(
            work, uki_recovery, kver,
            cmdline("a", "systemd.unit=stylos-recovery.target"),
        )

        # Drop mkosi's single-slot UKI and its loader entry; systemd-boot
        # discovers the three slot UKIs in EFI/Linux automatically.
        for line in esp.mdir("/EFI/Linux").splitlines():
            if line.endswith(".efi") and "stylos-" in line:
                esp.remove(f"/EFI/Linux/{line.rstrip('/').split('/')[-1]}")
        for line in esp.mdir("/loader/entries").splitlines():
            if line.endswith(".conf"):
                esp.remove(f"/loader/entries/{line.rstrip('/').split('/')[-1]}")

        esp.copy_in(uki_a, "/EFI/Linux/stylos-a.efi")
        # Slot B has no UKI until an update installs one.  A stale good-looking
        # stylos-b.efi would be selected after a failed counted B entry even
        # though sysupdate has already replaced the B root filesystem.
        # The 0 prefix sorts after slots A and B (systemd-boot lists entries in
        # reverse order), so a failed slot falls back to A rather than to the
        # recovery menu.
        esp.copy_in(uki_recovery, "/EFI/Linux/stylos-0-recovery.efi")
        esp.write(
            "default stylos-a.efi\ntimeout 2\n", "/loader/loader.conf",
            work, "loader.conf",
        )

        esp.write(grub_cfg(kver), "/grub/grub.cfg", work, "grub.cfg")
        env = work / "grubenv"
        run("grub-editenv", env, "create")
        run(
            "grub-editenv", env, "set",
            "stylos_default=a", "stylos_a_tries=0", "stylos_b_tries=0",
        )
        esp.copy_in(env, "/grub/grubenv")

        build_update_payload(image, root_a, work, out, kver)

    print(f"assemble: {image} has slots A/B, recovery and an update fixture")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
