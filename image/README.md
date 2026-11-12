# Stylos image (WP1)

One GPT disk image, bootable on both:

- UEFI → systemd-boot, with a unified kernel image in `EFI/Linux/`.
- Legacy BIOS → GRUB installed in the BIOS boot partition.

`mkosi` creates the partition table with systemd-repart: a 1 GiB ESP, a 1 MiB
BIOS boot partition and an ext4 root partition. WP7 replaces the default
layout with `image/mkosi.repart/` (ESP, root-a, root-b, data) and adds
sysupdate; nothing in the WP1 configuration depends on the single-root
layout.

## What runs

- `stylos-session.service`: `cage -d -- /usr/bin/stylos-shell` as `writer`
  on tty1. Cage's default (no `-s`) keeps VT switching disabled; `-d` hides
  client-side decorations so the single client owns the screen. Note that
  `cage -s` on cage 0.2 **allows** VT switching, so the `-s` in the first
  draft of the spec was dropped to keep rule 1.
- `getty@.service`, `serial-getty@.service`, `console-getty.service`,
  `autovt@.service`, `systemd-networkd*` and `systemd-timesyncd` are masked by
  `mkosi.postinst` (systemd presets cannot mask units); no VT has a login and
  NetworkManager is the only network manager.
- `stylos-rfkill.service` runs `rfkill block wifi` before NetworkManager.
- logind suspends on lid close (`HandleLidSwitch=suspend`).
- `/data/writing` exists and is owned by `writer`. It is a directory on the
  root filesystem until WP7 gives it a partition.

## Placeholder shell

`image/placeholder/stylos-shell.c` is compiled by `mkosi.build` into
`/usr/bin/stylos-shell`. It is a minimal GTK4 window with the word "Stylos".
WP2 replaces it with the real Rust shell. It is not a terminal; no terminal
emulator is installed.

## Boot test

The boot test does not use the QEMU guest agent: Debian's QEMU 10 build does
not ship the `guest-*` QMP commands.  `tests/boot/run.py` asserts offline on
the image filesystem and online through the serial console, then checks the
Cage screenshot.  No test-only daemon is installed in the image.

## Typst

Debian trixie does not package the `typst` CLI. `mkosi.prepare` downloads the
pinned upstream static musl build (version and SHA-256 in the script) and
installs it as `/usr/bin/typst`, alongside Debian's `pandoc`.
