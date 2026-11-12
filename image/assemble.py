#!/usr/bin/env python3
"""Finish the Stylos disk image after mkosi has assembled it.

mkosi builds one unified kernel image with its own command line.  Stylos needs
three (slot A, slot B and the recovery menu), each with a different root hash,
and the BIOS GRUB menu needs the same slots.  Doing this in a post-output
script keeps the slot-specific values next to the partition definitions instead
of spreading them through mkosi profiles.

The verity hash and signature partitions are created by systemd-repart (see
image/mkosi.repart); this script reads the per-slot root hash from repart's
JSON output, puts ``roothash=`` plus the fixed data/hash partition UUIDs into
the UKI command lines and GRUB entries, and signs the UKIs with the release
key when one is configured.

The script also writes the offline update fixture used by tests/boot/: a copy
of the root filesystem (with a failing unit), its verity hash/signature and a
UKI for slot B whose command line masks the session, so boot never reaches
boot-complete.target.
"""

from __future__ import annotations

import base64
import hashlib
import json
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
HASH_A_UUID = "55555555-5555-4555-8555-555555555555"
HASH_B_UUID = "66666666-6666-4666-8666-666666666666"
SIG_A_UUID = "77777777-7777-4777-8777-777777777777"
SIG_B_UUID = "88888888-8888-4888-8888-888888888888"

ROOT_UUID = {"a": ROOT_A_UUID, "b": ROOT_B_UUID}
HASH_UUID = {"a": HASH_A_UUID, "b": HASH_B_UUID}
SIG_UUID = {"a": SIG_A_UUID, "b": SIG_B_UUID}
SLOT_BY_UUID = {ROOT_A_UUID: "a", ROOT_B_UUID: "b"}

BASE_CMDLINE = (
    "console=tty0 console=ttyS0,115200 systemd.getty_auto=no "
    "ro rootfstype=erofs"
)
VERSION = os.environ.get("STYLOS_VERSION", "0.1.0")
UPDATE_VERSION = os.environ.get("STYLOS_UPDATE_VERSION", "0.2.0")

UKIFY = "/usr/bin/ukify"
STUB = "/usr/lib/systemd/boot/efi/linuxx64.efi.stub"
SYSTEMD_REPART = "/usr/bin/systemd-repart"
VERITYSETUP = "/usr/sbin/veritysetup"


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
                    "uuid": str(uuid.UUID(bytes_le=entry[16:32])),
                    "first": first,
                    "last": last,
                    "size": (last - first + 1) * 512,
                    "name": entry[56:128].decode("utf-16le").rstrip("\0"),
                }
            )
    return entries


def verity_parameters(image: Path, hash_part: dict) -> dict:
    """Read the verity superblock of a hash partition.

    mkosi's post-output sandbox has no loop devices, so copy the (small) hash
    partition to a temporary file before asking veritysetup about it.
    """
    with tempfile.NamedTemporaryFile(suffix=".verity") as copy:
        with image.open("rb") as source:
            source.seek(hash_part["first"] * 512)
            remaining = hash_part["size"]
            while remaining:
                chunk = source.read(min(remaining, 8 * 1024 * 1024))
                if not chunk:
                    break
                copy.write(chunk)
                remaining -= len(chunk)
        copy.flush()
        result = subprocess.run(
            [VERITYSETUP, "dump", copy.name], check=True, capture_output=True, text=True,
        )
    fields = {}
    for line in result.stdout.splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip()
    try:
        return {
            "salt": bytes.fromhex(fields["Salt"]),
            "data_blocks": int(fields["Data blocks"]),
            "data_block_size": int(fields["Data block size"]),
            "hash_block_size": int(fields["Hash block size"]),
        }
    except (KeyError, ValueError) as error:
        raise SystemExit(f"could not parse the verity superblock: {error}")


def data_root_hash(image: Path, data_part: dict, parameters: dict) -> str:
    """Compute the dm-verity root hash from the data partition.

    This is the same Merkle tree veritysetup builds: every data block is
    salted and hashed, the hashes are packed into hash blocks (zero padded),
    and each level is hashed again until one digest remains.  Computing it
    here avoids loop devices and a 2 GiB temporary copy inside mkosi's
    post-output sandbox.
    """
    salt = parameters["salt"]
    block_size = parameters["data_block_size"]
    hash_block_size = parameters["hash_block_size"]
    data_blocks = parameters["data_blocks"]
    digest_size = hashlib.sha256().digest_size
    per_block = hash_block_size // digest_size
    level: list[bytes] = []
    with image.open("rb") as source:
        source.seek(data_part["first"] * 512)
        for _ in range(data_blocks):
            block = source.read(block_size)
            if len(block) != block_size:
                raise SystemExit("data partition ended before its verity data blocks")
            level.append(hashlib.sha256(salt + block).digest())
    while len(level) > 1:
        next_level = []
        for index in range(0, len(level), per_block):
            block = b"".join(level[index:index + per_block])
            block = block.ljust(hash_block_size, b"\0")
            next_level.append(hashlib.sha256(salt + block).digest())
        level = next_level
    return level[0].hex()


def slot_root_hashes(image: Path, parts: list[dict]) -> dict[str, str]:
    hashes: dict[str, str] = {}
    for slot in ("a", "b"):
        data_part = next(
            (p for p in parts if p["uuid"] == ROOT_UUID[slot]),
            None,
        )
        hash_part = next(
            (p for p in parts if p["uuid"] == HASH_UUID[slot]),
            None,
        )
        if data_part is None or hash_part is None:
            raise SystemExit(f"slot {slot} has no verity data/hash partition")
        hashes[slot] = data_root_hash(
            image, data_part, verity_parameters(image, hash_part)
        )
    return hashes


def signing_arguments() -> list[str]:
    key = os.environ.get("STYLOS_VERITY_KEY")
    cert = os.environ.get("STYLOS_VERITY_CERT")
    if not key or not cert:
        return []
    return [
        "--signtool=sbsign",
        f"--secureboot-private-key={key}",
        f"--secureboot-certificate={cert}",
        "--sign-kernel",
    ]


def verity_format(root: Path, work: Path) -> tuple[Path, str]:
    """Make a dm-verity hash tree for ``root`` and return (hash, root hash)."""
    hash_file = work / (root.name + ".verity")
    result = subprocess.run(
        [VERITYSETUP, "format", "--hash=sha256", str(root), str(hash_file)],
        check=True, capture_output=True, text=True,
    )
    match = re.search(r"Root hash:\s*([0-9a-fA-F]+)", result.stdout)
    if not match:
        raise SystemExit(f"veritysetup did not report a root hash: {result.stdout}")
    return hash_file, match.group(1).lower()


def sign_roothash(roothash: str, work: Path) -> str:
    """Return the base64 PKCS#7 signature systemd-repart puts in sig partitions."""
    key = os.environ.get("STYLOS_VERITY_KEY")
    cert = os.environ.get("STYLOS_VERITY_CERT")
    if not key or not cert:
        raise SystemExit("STYLOS_VERITY_KEY/STYLOS_VERITY_CERT are required for signing")
    raw = work / "roothash.bin"
    raw.write_bytes(bytes.fromhex(roothash))
    signature = work / "roothash.sig"
    run(
        "openssl", "cms", "-sign", "-binary", "-noattr", "-nocerts",
        "-md", "sha256", "-signer", cert, "-inkey", key,
        "-in", raw, "-outform", "DER", "-out", signature,
    )
    return base64.b64encode(signature.read_bytes()).decode()


def certificate_fingerprint() -> str:
    cert = os.environ.get("STYLOS_VERITY_CERT")
    if not cert:
        raise SystemExit("STYLOS_VERITY_CERT is required for signing")
    der = subprocess.check_output(["openssl", "x509", "-in", cert, "-outform", "DER"])
    return hashlib.sha256(der).hexdigest()


def verity_signature(roothash: str, work: Path) -> bytes:
    document = {
        "rootHash": roothash,
        "certificateFingerprint": certificate_fingerprint(),
        "signature": sign_roothash(roothash, work),
    }
    return (json.dumps(document, separators=(",", ":")) + "\n").encode()


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
        *signing_arguments(),
        stdout=subprocess.DEVNULL,
    )


def cmdline(slot: str, roothash: str, extra: str = "") -> str:
    line = (
        f"{BASE_CMDLINE} roothash={roothash} "
        f"systemd.verity_root_data=PARTUUID={ROOT_UUID[slot]} "
        f"systemd.verity_root_hash=PARTUUID={HASH_UUID[slot]} "
        f"stylos.slot={slot}"
    )
    return f"{line} {extra}".strip()


def grub_cfg(kver: str) -> str:
    def entry(ident: str, title: str, slot: str, extra: str = "") -> str:
        cmd = (
            f"roothash=${{stylos_{slot}_roothash}} {BASE_CMDLINE} "
            f"systemd.verity_root_data=PARTUUID={ROOT_UUID[slot]} "
            f"systemd.verity_root_hash=PARTUUID={HASH_UUID[slot]} "
            f"stylos.slot={slot} {extra}"
        ).strip()
        return (
            f'menuentry "{title}" --id {ident} {{\n'
            f"    linux /stylos/{kver}/vmlinuz {cmd}\n"
            f"    initrd /stylos/initrd /stylos/{kver}/kernel-modules.initrd\n"
            "}\n"
        )

    # GRUB has no arithmetic and no EFI boot counter.  The counter is a
    # string ladder over grubenv; apply-update sets tries=3 and boot-complete
    # sets tries=good after a healthy boot.  GRUB decrements before it boots,
    # so a kernel/initrd panic still consumes a try.
    ladder = (
        'if [ "${{stylos_default}}" = "{slot}" ]; then\n'
        '    if [ "${{stylos_tries}}" = "3" ]; then set stylos_tries=2; '
        'save_env --file /grub/grubenv stylos_tries; set default=stylos-{slot}\n'
        '    elif [ "${{stylos_tries}}" = "2" ]; then set stylos_tries=1; '
        'save_env --file /grub/grubenv stylos_tries; set default=stylos-{slot}\n'
        '    elif [ "${{stylos_tries}}" = "1" ]; then set stylos_tries=0; '
        'save_env --file /grub/grubenv stylos_tries; set default=stylos-{slot}\n'
        '    elif [ "${{stylos_tries}}" = "good" ]; then set default=stylos-{slot}\n'
        "    fi\n"
        "fi\n"
    )
    return (
        "set timeout=2\n"
        "serial --unit=0 --speed=115200\n"
        "terminal_output serial console\n"
        "if [ -f /grub/grubenv ]; then\n"
        "    load_env --file /grub/grubenv stylos_default stylos_tries "
        "stylos_a_roothash stylos_b_roothash\n"
        "fi\n"
        "set default=stylos-a\n"
        + ladder.format(slot="b")
        + ladder.format(slot="a")
        + "\n"
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

    root_image = payload / f"stylos-root-{UPDATE_VERSION}.raw"
    run(
        "mkfs.erofs", "-zlz4hc,9", "-L", "stylos-root-b",
        root_image, bad_root,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    hash_file, roothash = verity_format(root_image, work)
    shutil.copyfile(hash_file, payload / f"stylos-verity-{UPDATE_VERSION}.raw")
    # Deliberately wrong root hash in the *signature partition*: the BIOS
    # rollback test needs a slot B that dies in the initrd (before userspace),
    # while the UEFI path still uses the correct roothash embedded in its UKI.
    (payload / f"stylos-verity-sig-{UPDATE_VERSION}.sig").write_bytes(
        verity_signature("0" * 64, work)
    )

    broken = cmdline("b", roothash, "systemd.mask=stylos-session.service")
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
    roothashes = slot_root_hashes(image, parts)
    esp = Esp(image, esp_part["first"] * 512)
    kver = kernel_versions(esp)

    with tempfile.TemporaryDirectory(dir=out) as tmp:
        work = Path(tmp)
        extract_sections(esp, work, kver)

        uki_a = work / "stylos-a.efi"
        uki_recovery = work / "stylos-recovery.efi"
        build_uki(work, uki_a, kver, cmdline("a", roothashes["a"]))
        build_uki(
            work, uki_recovery, kver,
            cmdline("a", roothashes["a"], "systemd.unit=stylos-recovery.target"),
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
            f"stylos_a_roothash={roothashes['a']}",
            f"stylos_b_roothash={roothashes['b']}",
        )
        esp.copy_in(env, "/grub/grubenv")

        build_update_payload(image, root_a, work, out, kver)

    print(f"assemble: {image} has slots A/B, verity, recovery and an update fixture")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
