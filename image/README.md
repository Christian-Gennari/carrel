# Stylos image

One GPT disk image, bootable on both:

- UEFI → Microsoft-signed shim → Debian-signed systemd-boot → Debian-signed
  kernel, with Boot Loader Specification Type #1 entries in
  `loader/entries/`. Secure Boot can stay on (see below).
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
| ESP | 1 GiB | vfat | shim, systemd-boot, boot entries, kernel/initrd |
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
hashes, `7777…`/`8888…` for signatures), so the boot entries and GRUB command
lines can name the verity devices across updates. The root hash itself is not
fixed: it is on the `options` line of the boot entry (and in grubenv for the
BIOS path). The slot
roles are still A/B; only the fixed UUIDs and `stylos.slot=` say which is
which.

`systemd-repart` creates the hash and signature partitions (`Verity=hash` /
`Verity=signature`) and signs the root hash with the release key. The entry's
kernel command line contains `roothash=`, `systemd.verity_root_data=PARTUUID=…` and
`systemd.verity_root_hash=PARTUUID=…`; `systemd-veritysetup-generator` in the
initrd sets up `/dev/mapper/root` before the root is mounted.

The ESP is 1 GiB rather than the 512 MiB in the first spec draft: mkosi's
build-time kernel image, the kernel/initrd pair and the update payloads need
the room. The root filesystem excludes `/boot` and `/efi`, so they exist only
on the ESP.

## State on data

- `/data/writing` — user Markdown, owned by `writer`.
- `/data/var` — bind-mounted over `/var` very early in boot.
- `/data/etc` — overlayfs upper layer; `/etc` is an overlay of the read-only
  root and this directory.

`stylos-data-init.service` runs before `sysinit.target` and sets all of this
up. It also creates `/data/writing/.stylos` as `writer` before `stylosd`
(root) creates its Git history inside it.

`stylos-grow-data.service` runs before `data.mount` and grows the data
partition to the disk size with systemd-repart; `x-systemd.growfs` in fstab
then grows the ext4 file system when `/data` is mounted. The installer grows
the partition before its first reboot.

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

## Secure Boot

A user never changes firmware settings: the image boots with Secure Boot on,
using only binaries that Debian signed. Nothing in the UEFI chain is signed
with a Stylos key.

```
firmware (Microsoft keys in db)
  └─ EFI/BOOT/BOOTX64.EFI       shim, Microsoft-signed (Debian shim-signed)
       └─ grubx64.efi           Debian-signed systemd-boot
            └─ loader/entries/*.conf  Type #1 entries
                 └─ stylos/<kver>/vmlinuz   Debian-signed kernel
                      └─ initrd (unsigned) → erofs root + dm-verity
```

- shim is the file the firmware starts. It carries Debian's CA as its vendor
  certificate and, by default, loads `grubx64.efi` from its own directory.
  Debian's signed systemd-boot is installed under that name, next to
  `mmx64.efi` (MokManager). The same three files are in `EFI/BOOT/` (removable
  media, the USB stick) and in `EFI/stylos/` (the firmware boot entry made by
  the installer). They come unchanged from the Debian packages `shim-signed`,
  `shim-helpers-amd64-signed` and `systemd-boot-efi-amd64-signed`;
  `mkosi.postinst` stages them and `assemble.py` places them on the ESP.
- systemd-boot checks the kernel with shim's `ShimLock` protocol, so the
  Debian-signed `vmlinuz` (from `linux-image-amd64`) passes without enrolling
  any key. Under Secure Boot systemd-boot disables its command-line editor
  (`editor no` is also in `loader.conf`) and the kernel is locked down
  (no hibernation, no unsigned modules, no `/dev/mem`). Stylos suspends, it
  does not hibernate.
- **What is not authenticated.** The initrd and the entry file with the
  `roothash=` are plain files on the FAT ESP, which Debian's chain does not
  sign. With physical access, someone can replace the initrd or the root hash
  on the ESP. This is the normal limit of the Debian chain (Debian's own
  initramfs is unsigned too). Closing it needs a measured/signed UKI with the
  Stylos key enrolled through MOK; that is out of scope for this release. The
  root file system itself is checked by dm-verity against the root hash.
- The installer registers `\EFI\stylos\shimx64.efi` with `efibootmgr` as the
  first boot entry. If the firmware refuses the entry, the disk still boots
  through `EFI/BOOT/BOOTX64.EFI`.
- Firmware that does not trust the Microsoft UEFI CA (some Chromebooks) is not
  supported, as in `spec/04-hardware.md`.
- QEMU test: `OVMF_CODE_4M.ms.fd` / `OVMF_VARS_4M.ms.fd` from Debian's `ovmf`
  package (Microsoft keys enrolled, Secure Boot enforcing, SMM-protected
  variable store). `tests/boot/run.py` checks the kernel message
  `secureboot: Secure boot enabled`, lockdown and the guest's `SecureBoot`
  EFI variable (logged by `stylos-boot-complete` as
  `STYLOS-BOOT: secureboot=1`).

## Boot menu and A/B updates

`image/assemble.py` (run by `mkosi.postoutput`) replaces mkosi's single
throw-away UKI with Boot Loader Specification Type #1 entries
(`loader/entries/`, kernel and initrd shared in `stylos/` on the ESP):

- `stylos-a.conf` — active slot, default. `loader.conf` has
  `default stylos-a.conf`.
- `stylos-recovery.conf` — `systemd.unit=stylos-recovery.target`.
- `stylos-b.conf` is *not* installed initially. It appears only after an
  update, so a failed B entry can never fall back to a stale B kernel.

Each entry's `options` line has the slot's `roothash=` and verity devices.
`sort-key` keeps the menu order A, B, recovery, so a missing slot B falls
back to A. It also writes the BIOS `grub.cfg` and `grubenv`.

Updates use `systemd-sysupdate` (transfer files in
`/usr/lib/stylos/sysupdate/`). A staged payload lives in
`/data/.stylos/update/payload/` and is applied by
`stylos-update.service` before the session starts:

1. sysupdate writes the older root partition, its verity hash tree, the
   signed verity root hash and a counted boot entry
   `loader/entries/stylos-b-<version>+3.conf`.
2. `apply-update` sets the boot-counted entry as the default (systemd-boot
   matches the entry ID without the `+3` suffix), updates the BIOS grubenv
   root hash for the new slot and reboots.
3. `stylos-boot-complete.service` runs after the session and checks for
   failed units. If the boot is healthy it renames the counted entry to
   `stylos-b.conf` and makes it the default.
4. If the boot is unhealthy, the service leaves the counter alone. On the
   third failed attempt the OS removes the exhausted entry and reboots into
   the previous slot. systemd-boot 257 does not remove a `+0-N` entry by
   itself, so this last step is deliberately done by the OS.

The kernel and initrd in `stylos/` are shared by both slots (as for BIOS): a
release that changes the kernel must also update those files; the current
release pipeline changes the root filesystem and the entries.

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
  it to systemd-repart, which signs the verity root hashes. It installs the public keyring at
  `/usr/lib/systemd/import-pubring.gpg` and the certificate at
  `/usr/share/stylos/keys/verity.crt`.
- `STYLOS_UPDATE_URL` (default `https://updates.stylos.example/stylos/`) is
  written to `/usr/lib/stylos/update-url`.
- `make release` takes the running root from the image, bumps its version,
  builds `out/release/` with root + verity + signature + boot entry for both
  slots, writes `SHA256SUMS` and signs it as `SHA256SUMS.gpg` with the update
  key. It also builds the file users flash (next section).

The daemon checks and downloads updates only inside a sync session's radio
window. `UpdateStatus()` returns `none`, `available <version>`,
`downloading <pct>`, `ready <version>` or `error <text>`; `ApplyUpdate()`
stages the verified payload and starts `stylos-update.service`. `stylosd
update --url …` is the manual equivalent used by the QEMU online test.

## Release file

`make release` writes, next to the update payloads, `out/dist/`:

| File | What |
|---|---|
| `stylos-<version>.img.xz` | The disk image for users to flash (xz-compressed raw image). |
| `stylos-<version>.img.xz.sha256` | `sha256sum` line for the `.img.xz`. |
| `stylos-<version>.img.xz.asc` | Detached, armoured GPG signature of the `.img.xz` by the release (update) key. |

`<version>` is the `ImageVersion` the image was built with
(`image/mkosi.conf`); `RELEASE_VERSION` is only the version of the online
update payloads in `out/release/`. To ship 1.0, set both to 1.0 and build.

The image is **shrunk to its content**: the data partition, which is empty in
a fresh image, is recreated at 256 MiB and the image file ends right after it.
The ESP and the two root slots keep their size (sysupdate needs two equal
slots); their unused parts are zeros and compress to nothing. The result is a
raw image of about 5.3 GiB that compresses to a few hundred MiB.

The installed system grows the data partition by itself:

- Flashed to a USB stick, the first boot runs `stylos-grow-data.service`
  (systemd-repart moves the backup GPT header to the end of the stick and
  grows the data partition), then `x-systemd.growfs` in fstab grows the ext4
  file system when `/data` is mounted. **Try Stylos** therefore has the whole
  stick as writing space.
- **Install to this computer** copies the stick to the internal disk with `dd`
  and grows the data partition there before the reboot.

### Verify a download

```sh
sha256sum -c stylos-<version>.img.xz.sha256
gpg --import update-pubring.gpg       # image/keys/update-pubring.gpg in this repository
gpg --verify stylos-<version>.img.xz.asc stylos-<version>.img.xz
```

### Flash

Use Fedora Media Writer, balenaEtcher, Rufus (choose *DD Image mode*) or
`xz -dc stylos-<version>.img.xz | sudo dd of=/dev/sdX bs=4M status=progress
conv=fsync`. The root README has the steps. Docker is only the build
environment; users never need it.

### Gate

`tests/boot/run.py --scenario release --release-image out/dist/stylos-<version>.img.xz`
decompresses the file onto a fresh 16 GiB virtual USB disk with `dd`, boots it
in QEMU on UEFI, UEFI with Secure Boot enforcing and BIOS, chooses **Try
Stylos**, reaches the editor and asserts that the data partition grew to fill
the stick.

## Recovery

Holding a key at the systemd-boot/GRUB menu and choosing **Stylos recovery**
boots `stylos-recovery.conf`, i.e. `systemd.unit=stylos-recovery.target`.
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
When the computer booted in UEFI mode, the installer then makes Stylos the
first UEFI boot entry with `efibootmgr` (an entry `Stylos` for
`\EFI\stylos\shimx64.efi`, put first in `BootOrder`; an older Stylos entry is
removed). Booting the installed disk is not removable, so the installer does
not run again.

## Boot test

`make test-boot` runs `tests/boot/run.py` in QEMU (UEFI, UEFI with Secure
Boot, BIOS) with QMP:

- offline checks of the GPT, ESP, both roots, verity hash/signature
  partitions, the signed shim/systemd-boot/kernel and the boot entries, real
  binaries, units, fonts, fstab, keys, oauth
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
  `ApplyUpdate` over D-Bus, reboot into 0.2.0 and assert the new slot label;
- `secureboot`: boot the image, install it to a blank disk and boot the
  installed disk with OVMF's Microsoft keys enrolled and Secure Boot enforcing;
  assert the installer made Stylos the first UEFI boot entry;
- `update-secureboot`: update and forced rollback on that installed disk;
- `release`: flash `out/dist/stylos-<version>.img.xz` onto a fresh virtual USB
  disk and try Stylos on UEFI, Secure Boot and BIOS.

Screenshots are written to `out/screenshot-*.png`.

## Known limitations

- Secure Boot: the initrd and the entry's `roothash=` are not authenticated
  by Debian's chain (see Secure Boot above). The real release key and the
  tier 1 hardware checks are part of WP10.
- Both boot paths use the shared ESP kernel/initrd. A release that changes
  the kernel must also update those files; the current release pipeline
  changes the root filesystem and boot entries, not the shared kernel.
- The flashable image is named after `ImageVersion`, not `RELEASE_VERSION`
  (see Release file).
- The `/etc` overlay is mounted by an early systemd service, after the
  manager has read the unit files from the read-only lower layer. Runtime
  writes to `/etc` persist on data, but early systemd configuration changes
  are only read from the lower layer on the next boot.
- `stylos-online-update-test.service` is a dormant QEMU test hook: it only
  runs when `/data/.stylos/update/test-url` exists, which production never
  creates.
