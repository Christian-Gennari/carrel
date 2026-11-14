#!/usr/bin/env python3
"""Build the signed online-update payloads in ``out/release/``.

The image must already exist (``make image``).  This script takes the running
slot's root filesystem, bumps its version, creates a matching dm-verity hash
tree and signature, writes the slot A and B boot entries, and writes a
``SHA256SUMS`` manifest plus its detached GPG signature.  A device built with
the matching public keyring verifies the manifest with systemd-sysupdate.

Private keys never enter the output or the repository.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import assemble  # noqa: E402


def run(*args: object, **kwargs: object) -> subprocess.CompletedProcess:
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def write_os_release(root: Path, version: str) -> None:
    release = root / "usr/lib/os-release"
    if not release.exists():
        raise SystemExit(f"{release} is missing from the extracted root")
    lines = []
    for line in release.read_text().splitlines():
        if line.startswith(("VERSION_ID=", "IMAGE_VERSION=", "BUILD_ID=")):
            continue
        lines.append(line)
    lines.append(f"VERSION_ID={version}")
    lines.append(f"IMAGE_VERSION={version}")
    release.write_text("\n".join(lines) + "\n")
    marker = root / "usr/share/carrel/release-version"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(version + "\n")


def sign_manifest(manifest: Path, secret: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="carrel-release-gpg-") as home:
        os.chmod(home, 0o700)
        env = dict(os.environ, GNUPGHOME=home)
        run("gpg", "--batch", "--import", secret, env=env)
        run(
            "gpg", "--batch", "--yes", "--armor", "--detach-sign",
            "--output", manifest.with_name(manifest.name + ".gpg"), manifest,
            env=env,
        )


def read_kernel_file(path: Path) -> str:
    """The first non-comment line of a kernel-version file."""
    if not path.is_file():
        return ""
    for line in path.read_text().splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            return line
    return ""


# The shipped image keeps a small data partition; the installed or flashed
# system grows it to fill the disk on first boot (carrel-grow-data.service
# runs systemd-repart, fstab's x-systemd.growfs grows the file system).
DATA_SHIPPED_BYTES = 256 * 1024 * 1024
GAP_BYTES = 1024 * 1024  # room for the backup GPT after the data partition


def sign_file(path: Path, secret: Path) -> Path:
    """Detached, ASCII-armoured signature ``<path>.asc`` by the release key."""
    with tempfile.TemporaryDirectory(prefix="carrel-release-gpg-") as home:
        os.chmod(home, 0o700)
        env = dict(os.environ, GNUPGHOME=home)
        run("gpg", "--batch", "--import", secret, env=env,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        signature = path.with_name(path.name + ".asc")
        signature.unlink(missing_ok=True)
        run("gpg", "--batch", "--yes", "--armor", "--detach-sign",
            "--output", signature, path, env=env)
    return signature


def shrink_image(image: Path, work: Path) -> None:
    """Cut a copy of the built image down to its content.

    Only the data partition is shrunk: it is empty in a fresh image, so it is
    recreated at DATA_SHIPPED_BYTES, the image file ends right after it, and
    the GPT is rewritten there.  The ESP and both root slots keep their size
    (sysupdate needs two root slots of the same size).  The empty parts of
    those compress to almost nothing.
    """
    parts = assemble.read_gpt(work)
    data = next(p for p in parts if p["uuid"] == assemble.DATA_UUID)
    dump = subprocess.run(
        ["sfdisk", "--dump", str(work)], check=True, capture_output=True, text=True,
    ).stdout
    lines = []
    for line in dump.splitlines():
        if line.startswith(("last-lba:", "device:")):
            continue
        if f"start={data['first']}," in line.replace(" ", ""):
            line = re.sub(
                r"size=\s*\d+", f"size={DATA_SHIPPED_BYTES // 512}", line, count=1,
            )
        lines.append(line)
    total = data["first"] * 512 + DATA_SHIPPED_BYTES + GAP_BYTES
    run("truncate", "-s", total, work)
    run("sfdisk", "--no-reread", "--no-tell-kernel", work,
        input="\n".join(lines) + "\n", text=True, stdout=subprocess.DEVNULL)
    run("mkfs.ext4", "-q", "-F", "-L", "carrel-data",
        "-E", f"offset={data['first'] * 512}", work, f"{DATA_SHIPPED_BYTES // 1024}k")
    after = assemble.read_gpt(work)
    new_data = next(p for p in after if p["uuid"] == assemble.DATA_UUID)
    if new_data["size"] != DATA_SHIPPED_BYTES or new_data["first"] != data["first"]:
        raise SystemExit("shrinking the data partition did not give the expected table")


def build_dist(image: Path, dist: Path, version: str, secret: Path) -> Path:
    """``carrel-<version>.img.xz`` + ``.sha256`` + ``.asc`` for users to flash."""
    if dist.exists():
        shutil.rmtree(dist)
    dist.mkdir(parents=True)
    raw = dist / f"carrel-{version}.img"
    run("cp", "--sparse=always", image, raw)
    shrink_image(image, raw)
    archive = dist / f"carrel-{version}.img.xz"
    with archive.open("wb") as target:
        run("xz", "-T4", "-6", "-c", raw, stdout=target)
    raw.unlink()
    digest = hashlib.sha256()
    with archive.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    (dist / f"{archive.name}.sha256").write_text(f"{digest.hexdigest()}  {archive.name}\n")
    sign_file(archive, secret)
    return archive


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--version", default=assemble.VERSION)
    parser.add_argument("--verity-key", required=True, type=Path)
    parser.add_argument("--verity-cert", required=True, type=Path)
    parser.add_argument("--update-secret", required=True, type=Path)
    parser.add_argument("--dist", type=Path,
                        help="also write carrel-<image-version>.img.xz, .sha256 and "
                             ".asc to this directory")
    parser.add_argument("--image-version", default=assemble.VERSION,
                        help="version of the built image (file name of the .img.xz)")
    parser.add_argument("--previous-kernel",
                        help="kernel version of the previous field release "
                             "(default: image/previous-release-kernel)")
    parser.add_argument("--allow-kernel-change", action="store_true",
                        help="allow a payload whose kernel differs from the field "
                             "kernel; only for a release whose predecessors support "
                             "the kernel transfer")
    parser.add_argument("--kernel-version",
                        help="test fixture only: label the kernel files and entries "
                             "with this version instead of the image's kernel")
    args = parser.parse_args()

    image = args.image.resolve()
    out = args.out.resolve()
    if not image.exists():
        print(f"image missing: {image}", file=sys.stderr)
        return 2
    for path in (args.verity_key, args.verity_cert, args.update_secret):
        if not path.exists():
            print(f"missing key: {path}", file=sys.stderr)
            return 2

    os.environ["CARREL_VERITY_KEY"] = str(args.verity_key)
    os.environ["CARREL_VERITY_CERT"] = str(args.verity_cert)

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    parts = assemble.read_gpt(image)
    esp_part = next(p for p in parts if p["type"] == assemble.ESP_GUID)
    root_a = next(p for p in parts if p["uuid"] == assemble.ROOT_A_UUID)
    esp = assemble.Esp(image, esp_part["first"] * 512)
    kver = assemble.kernel_versions(esp)
    current_kernel = args.kernel_version or kver

    previous_kernel = args.previous_kernel
    if previous_kernel is None:
        previous_kernel = read_kernel_file(
            Path(__file__).resolve().parent / "previous-release-kernel")
    if previous_kernel and current_kernel != previous_kernel:
        if not args.allow_kernel_change:
            print(
                f"release: refusing a {current_kernel} payload: the field kernel is "
                f"{previous_kernel}.\n"
                "Devices on releases before 0.2.2 install only the root, verity, "
                "signature and\nboot entry, so they can only boot the kernel they "
                "already have on their ESP.  Pass\n--allow-kernel-change only when "
                "every release still in the field has the kernel\ntransfer (0.2.2 "
                "and later).",
                file=sys.stderr,
            )
            return 2
        print(f"release: kernel {current_kernel} differs from the field kernel "
              f"{previous_kernel}; --allow-kernel-change was passed")

    with tempfile.TemporaryDirectory(dir=out) as tmp:
        work = Path(tmp)
        root = work / "root"
        run(
            "fsck.erofs", f"--offset={root_a['first'] * 512}",
            f"--extract={root}", "--overwrite", "--preserve", image,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        write_os_release(root, args.version)
        root_image = work / "carrel-root.raw"
        run(
            "mkfs.erofs", "-zlz4hc,9", "-L", "carrel-root",
            root_image, root,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        hash_file, roothash = assemble.verity_format(root_image, work)
        signature = assemble.verity_signature(roothash, work)

        shutil.copyfile(root_image, out / f"carrel-root-{args.version}.raw")
        shutil.copyfile(hash_file, out / f"carrel-verity-{args.version}.raw")
        (out / f"carrel-verity-sig-{args.version}.sig").write_bytes(signature)
        # The kernel and its initrds travel as one archive.  It unpacks into
        # /carrel/<kernel-version>/ on the ESP, where the boot entry names the
        # files; the kernel transfer in the image is a directory transfer.
        kernel_files = work / "kernel"
        kernel_files.mkdir()
        for name, esp_path in (
            ("vmlinuz", f"/carrel/{kver}/vmlinuz"),
            ("kernel-modules.initrd", f"/carrel/{kver}/kernel-modules.initrd"),
            ("initrd", f"/carrel/{kver}/initrd"),
        ):
            esp.copy_out(esp_path, kernel_files / name)
        run("tar", "-C", kernel_files, "-cf",
            out / f"carrel-kernel-{current_kernel}.tar",
            "vmlinuz", "kernel-modules.initrd", "initrd")
        for slot in ("a", "b"):
            (out / f"carrel-{slot}-{args.version}.conf").write_text(
                assemble.entry_conf(
                    slot, "Carrel", args.version, current_kernel, roothash,
                    assemble.SORT_KEY[slot],
                )
            )

    files = sorted(
        path for path in out.iterdir()
        if path.is_file() and path.name != "SHA256SUMS"
    )
    manifest = out / "SHA256SUMS"
    lines = []
    for path in files:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        lines.append(f"{digest}  {path.name}\n")
    manifest.write_text("".join(lines))
    sign_manifest(manifest, args.update_secret)

    # Not part of SHA256SUMS: sysupdate never downloads it.  It records the
    # kernel of this release so the next release's guard and support can read
    # it from the GitHub assets.
    (out / "KERNEL").write_text(current_kernel + "\n")

    (out / "README").write_text(
        f"""Carrel {args.version} online update

Attach these files (flat, no directories) to the GitHub Release, or serve the
directory at the URL configured as CARREL_UPDATE_URL when the image was built.
systemd-sysupdate reads SHA256SUMS, verifies SHA256SUMS.gpg against
/usr/lib/systemd/import-pubring.gpg in the image, then downloads only the
files for the inactive slot.

Files:
  carrel-root-{args.version}.raw          root filesystem
  carrel-verity-{args.version}.raw        dm-verity hash tree
  carrel-verity-sig-{args.version}.sig    signed verity root hash
  carrel-a-{args.version}.conf             slot A boot entry
  carrel-b-{args.version}.conf             slot B boot entry
  carrel-kernel-{current_kernel}.tar       kernel, module initrd and initrd
                                          (unpacks into /carrel/{current_kernel}/)
  KERNEL                                  kernel version (metadata, not downloaded)

The root hash is {roothash}.  The kernel is {current_kernel}.
"""
    )
    print(f"release: {out} ({args.version}, roothash {roothash})")
    if args.dist is not None:
        archive = build_dist(image, args.dist.resolve(), args.image_version,
                             args.update_secret)
        print(f"release: {archive} ({archive.stat().st_size // 1024**2} MiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
