# Stylos image

One GPT disk image, bootable on both:

- UEFI → systemd-boot, with unified kernel images in `EFI/Linux/`.
- Legacy BIOS → GRUB in the BIOS boot partition, with the same kernel and
  initrd from the ESP.

`make image` builds it with mkosi from this directory. The output is
`out/image/stylos.img.raw` (and a `stylos.img` symlink). It is the USB
installer *and* the installed system; the installer only starts when the
running root is on removable/USB media.

## Partition layout

systemd-repart creates the table from `mkosi.repart/`:

| Partition | Size | Filesystem | Purpose |
|---|---|---|---|
| ESP | 1 GiB | vfat | systemd-boot, UKIs, GRUB kernel/initrd |
| BIOS boot | 1 MiB | — | GRUB i386-pc core image |
| root slot A | 2 GiB | erofs + dm-verity | read-only active system |
| verity A | 64 MiB | raw | hash tree for slot A |
| verity A signature | 16 KiB | raw | signed root hash (JSON) |
| root slot B | 2 GiB | erofs + dm-verity | read-only spare/update slot |
| verity B | 64 MiB | raw | hash tree for slot B |
| verity B signature | 16 KiB | raw | signed root hash (JSON) |
| data | ≥ 2 GiB | ext4 | grows to fill the disk |

The two root slots are labelled `stylos-root-<version>`
(`stylos-root-0.1.0` and `stylos-root-0` in a fresh image). The version in the
label is what systemd-sysupdate compares; both slots deliberately share the
same label prefix so sysupdate can pick the older one. The data and hash
partition UUIDs are fixed (`1111…`/`2222…` for roots, `5555…`/`6666…` for
hashes, `7777…`/`8888…` for signatures), so the UKI and GRUB command lines can
name the verity devices across updates. The root hash itself is not fixed: it
is in the signed UKI command line (and in grubenv for the BIOS path). The slot
roles are still A/B; only the fixed UUIDs and `stylos.slot=` say which is
which.

`systemd-repart` creates the hash and signature partitions (`Verity=hash` /
`Verity=signature`) and signs the root hash with the release key. The UKI
command line contains `roothash=`, `systemd.verity_root_data=PARTUUID=…` and
`systemd.verity_root_hash=PARTUUID=…`; `systemd-veritysetup-generator` in the
initrd sets up `/dev/mapper/root` before the root is mounted.

The ESP is 1 GiB rather than the 512 MiB in the first spec draft: two slot
UKIs, the recovery UKI and the GRUB kernel/initrd do not fit in 512 MiB. The
root filesystem excludes `/boot` and `/efi`, so they exist only on the ESP.

## State on data

- `/data/writing` — user Markdown, owned by `writer`.
- `/data/var` — bind-mounted over `/var` very early in boot.
- `/data/etc` — overlayfs upper layer; `/etc` is an overlay of the read-only
  root and this directory.

`stylos-data-init.service` runs before `sysinit.target` and sets all of this
up. It also creates `/data/writing/.stylos` as `writer` before `stylosd`
(root) creates its Git history inside it.

`stylos-grow-data.service` runs before `data.mount` and grows the data
partition to the disk size with systemd-repart. The installer does the same
before its first reboot.

## Binaries

`make image` first runs `cargo build --release -p stylos-shell -p stylosd` in
the dev container (with the shared cargo cache), then `mkosi.build` installs
`out`-independent build products from `$SRCDIR/target/release`. Nothing built
is committed. `mkosi.build` also installs `stylosd/systemd/stylosd.service`
and the D-Bus policy from the `stylosd` crate, so those files have one source.

`STYLOS_OAUTH_TOML` (default `~/.config/stylos/oauth.toml`) is copied to
`/usr/share/stylos/oauth.toml` at build time if it exists. `dev/run.sh`
places a 0600 copy in the repo for the duration of the container run and
removes it afterwards; the file is gitignored and never committed. Without
it the image still builds (Google Drive pairing is unavailable).

Fonts: Inter and JetBrains Mono come from Debian (`fonts-inter`,
`fonts-jetbrains-mono`). Source Serif 4 is not packaged in trixie, so
`mkosi.prepare` downloads the pinned OFL release (version and SHA-256 in the
script) exactly like the pinned Typst binary.

## Boot menu and A/B updates

`image/assemble.py` (run by `mkosi.postoutput`) replaces mkosi's single UKI
with:

- `stylos-a.efi` — active slot, default.
- `stylos-0-recovery.efi` — `systemd.unit=stylos-recovery.target`.
- `stylos-b.efi` is *not* installed initially. It appears only after an
  update, so a failed B entry can never fall back to a stale B kernel.

It also writes the BIOS `grub.cfg` and `grubenv`.

Updates use `systemd-sysupdate` (transfer files in
`/usr/lib/stylos/sysupdate/`). A staged payload lives in
`/data/.stylos/update/payload/` and is applied by
`stylos-update.service` before the session starts:

1. sysupdate writes the older root partition, its verity hash tree, the
   signed verity root hash and a counted UKI
   `EFI/Linux/stylos-b-<version>+3.efi`.
2. `apply-update` sets the boot-counted entry as the default (systemd-boot
   matches the entry ID without the `+3` suffix), updates the BIOS grubenv
   root hash for the new slot and reboots.
3. `stylos-boot-complete.service` runs after the session and checks for
   failed units. If the boot is healthy it renames the counted UKI to
   `stylos-b.efi` and makes it the default.
4. If the boot is unhealthy, the service leaves the counter alone. On the
   third failed attempt the OS removes the exhausted entry and reboots into
   the previous slot. systemd-boot 257 does not remove a `+0-N` entry by
   itself, so this last step is deliberately done by the OS.

GRUB (BIOS) has no EFI boot counter. `grub.cfg` reads `stylos_default`,
`stylos_<slot>_tries` and `stylos_<slot>_roothash` from `grubenv`, decrements
the counter on each boot and switches to the other slot when it reaches zero.
`apply-update` reads the new root hash from the signed verity signature
partition, so the BIOS command line stays authenticated too.
`stylos-boot-complete` resets the counter after a healthy boot. The QEMU gate
now exercises update + forced rollback on both UEFI and BIOS.

## Release keys and signed updates

Private keys never enter git:

- `make keys` creates `~/.config/stylos/keys/verity.key` and
  `~/.config/stylos/keys/update-secret.gpg` (both 0600), plus the matching
  public certificate and GPG keyring in `image/keys/`. Commit only
  `image/keys/`.
- `make image` reads the private key path from `STYLOS_KEY_DIR` /
  `VERITY_KEY` (the dev container mounts the host config directory) and passes
  it to systemd-repart and ukify. It installs the public keyring at
  `/usr/lib/systemd/import-pubring.gpg` and the certificate at
  `/usr/share/stylos/keys/verity.crt`.
- `STYLOS_UPDATE_URL` (default `https://updates.stylos.example/stylos/`) is
  written to `/usr/lib/stylos/update-url`.
- `make release` takes the running root from the image, bumps its version,
  builds `out/release/` with root + verity + signature + UKI for both slots,
  writes `SHA256SUMS` and signs it as `SHA256SUMS.gpg` with the update key.

The daemon checks and downloads updates only inside a sync session's radio
window. `UpdateStatus()` returns `none`, `available <version>`,
`downloading <pct>`, `ready <version>` or `error <text>`; `ApplyUpdate()`
stages the verified payload and starts `stylos-update.service`. `stylosd
update --url …` is the manual equivalent used by the QEMU online test.

## Recovery

Holding a key at the systemd-boot/GRUB menu and choosing **Stylos recovery**
boots `stylos-0-recovery.efi`, i.e. `systemd.unit=stylos-recovery.target`.
`/usr/lib/stylos/recovery` is a whiptail menu with exactly four actions:

- Boot previous system (rewrites the loader default / grubenv and reboots)
- Copy all writing to a USB drive
- Reset settings (keeps writing)
- Power off

There is no shell or getty in the recovery target.

## Installer

When booted from a USB disk (removable or `TRAN=usb`),
`stylos-installer.service` shows **Try Stylos** / **Install to this
computer** on tty1 before the session. Installing lists internal disks,
requires the writer to type `ERASE`, copies the running USB disk to the
target with `dd`, grows the data partition with systemd-repart and reboots.
Booting the installed disk is not removable, so the installer does not run
again.

## Boot test

`make test-boot` runs `tests/boot/run.py` in QEMU (UEFI + BIOS) with QMP:

- offline checks of the GPT, ESP, both roots, verity hash/signature
  partitions, signed UKIs, real binaries, units, fonts, fstab, keys, oauth
  and the update fixture;
- boot UEFI and BIOS to the real `stylos-shell`, complete first run, type a
  line and screenshot the editor; OCR asserts the no-battery line has no
  percentage;
- install the USB image to a blank 16 GiB disk and boot that disk;
- screenshot the recovery menu, attach a virtual USB disk, choose "Copy all
  writing to USB drive" and assert the files are on that disk;
- type into the editor with QMP `send-key`, `system_reset` after 2 s, boot
  again and assert the text reached `/data/writing`;
- stage the offline update, apply it with sysupdate, make the new slot fail
  three times and assert the previous slot boots, on UEFI **and** BIOS;
- serve `out/release/` over HTTP inside QEMU, reject a bad manifest
  signature, then stage the good release through the daemon, call
  `ApplyUpdate` over D-Bus, reboot into 0.2.0 and assert the new slot label.

Screenshots are written to `out/screenshot-*.png`.

## Known limitations

- Secure Boot itself is not enabled in QEMU; the UKIs and verity root hashes
  are signed with the development release key, but firmware enforcement and
  the real release key are part of WP10.
- The BIOS path reuses the ESP kernel/initrd for both slots. A release that
  changes the kernel must also update those files; the current release
  pipeline changes the root filesystem and UKI, not the shared BIOS kernel.
- The `/etc` overlay is mounted by an early systemd service, after the
  manager has read the unit files from the read-only lower layer. Runtime
  writes to `/etc` persist on data, but early systemd configuration changes
  are only read from the lower layer on the next boot.
- `stylos-online-update-test.service` is a dormant QEMU test hook: it only
  runs when `/data/.stylos/update/test-url` exists, which production never
  creates.
