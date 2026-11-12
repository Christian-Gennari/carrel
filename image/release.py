#!/usr/bin/env python3
"""Build the signed online-update payloads in ``out/release/``.

The image must already exist (``make image``).  This script takes the running
slot's root filesystem, bumps its version, creates a matching dm-verity hash
tree and signature, builds signed slot A and B UKIs, and writes a
``SHA256SUMS`` manifest plus its detached GPG signature.  A device built with
the matching public keyring verifies the manifest with systemd-sysupdate.

Private keys never enter the output or the repository.
"""

from __future__ import annotations

import argparse
import hashlib
import os
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
    marker = root / "usr/share/stylos/release-version"
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(version + "\n")


def sign_manifest(manifest: Path, secret: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="stylos-release-gpg-") as home:
        os.chmod(home, 0o700)
        env = dict(os.environ, GNUPGHOME=home)
        run("gpg", "--batch", "--import", secret, env=env)
        run(
            "gpg", "--batch", "--yes", "--armor", "--detach-sign",
            "--output", manifest.with_name(manifest.name + ".gpg"), manifest,
            env=env,
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--version", default="0.2.0")
    parser.add_argument("--verity-key", required=True, type=Path)
    parser.add_argument("--verity-cert", required=True, type=Path)
    parser.add_argument("--update-secret", required=True, type=Path)
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

    os.environ["STYLOS_VERITY_KEY"] = str(args.verity_key)
    os.environ["STYLOS_VERITY_CERT"] = str(args.verity_cert)

    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    parts = assemble.read_gpt(image)
    esp_part = next(p for p in parts if p["type"] == assemble.ESP_GUID)
    root_a = next(p for p in parts if p["uuid"] == assemble.ROOT_A_UUID)
    esp = assemble.Esp(image, esp_part["first"] * 512)
    kver = assemble.kernel_versions(esp)

    with tempfile.TemporaryDirectory(dir=out) as tmp:
        work = Path(tmp)
        root = work / "root"
        run(
            "fsck.erofs", f"--offset={root_a['first'] * 512}",
            f"--extract={root}", "--overwrite", "--preserve", image,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        write_os_release(root, args.version)
        assemble.extract_sections(esp, work, kver)
        # Use the new root's os-release in the UKI's .osrel section too.
        shutil.copyfile(root / "usr/lib/os-release", work / "osrel")

        root_image = work / "stylos-root.raw"
        run(
            "mkfs.erofs", "-zlz4hc,9", "-L", "stylos-root",
            root_image, root,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        hash_file, roothash = assemble.verity_format(root_image, work)
        signature = assemble.verity_signature(roothash, work)

        shutil.copyfile(root_image, out / f"stylos-root-{args.version}.raw")
        shutil.copyfile(hash_file, out / f"stylos-verity-{args.version}.raw")
        (out / f"stylos-verity-sig-{args.version}.sig").write_bytes(signature)
        for slot in ("a", "b"):
            assemble.build_uki(
                work,
                out / f"stylos-{slot}-{args.version}.efi",
                kver,
                assemble.cmdline(slot, roothash),
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

    (out / "README").write_text(
        f"""Stylos {args.version} online update

Serve this directory at the URL configured as STYLOS_UPDATE_URL when the
image was built.  systemd-sysupdate reads SHA256SUMS, verifies
SHA256SUMS.gpg against /usr/lib/systemd/import-pubring.gpg in the image, then
downloads only the files for the inactive slot.

Files:
  stylos-root-{args.version}.raw          root filesystem
  stylos-verity-{args.version}.raw        dm-verity hash tree
  stylos-verity-sig-{args.version}.sig    signed verity root hash
  stylos-a-{args.version}.efi              signed slot A UKI
  stylos-b-{args.version}.efi              signed slot B UKI

The root hash is {roothash}.
"""
    )
    print(f"release: {out} ({args.version}, roothash {roothash})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
