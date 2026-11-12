# Stylos

Stylos is a free operating system that turns an old laptop into a dedicated,
distraction-free writing device, in the spirit of a Freewrite.

Open the lid and the cursor is on screen, at the last word you wrote. There is
no desktop, no browser, no notifications and no other app. Text is saved all
the time. One key sends your drafts to Nostos Cloud or to your own server, then
the network goes off again.

Project homepage: <https://stylos.nostos.page>.

Stylos works best with [Nostos](https://nostos.page): drafts appear in the
Nostos Writing Studio, and your highlights, notes and passages are searchable
on the device, offline, and insert as live citations.

## Status

Pre-alpha. Built mostly by AI agents against the specs in [`spec/`](spec/).

## Repository

| Path | What |
|---|---|
| `spec/` | Product rules, architecture, protocol, UI and phase plan. Read first. |
| `dev/` | Docker dev container (Debian 13 toolchain: Rust, GTK4, mkosi, QEMU). |
| `image/` | mkosi configuration for the bootable image and A/B updates. |
| `shell/` | `stylos-shell`: the Rust + GTK4 app (editor, files, settings, quotes, sync UI). |
| `stylosd/` | `stylosd`: Rust system daemon (history, sync, radio, updates). |
| `protocol/` | `stylos-protocol`: shared Rust crate with sync protocol types. |
| `server/` | `stylos-server`: self-hosted sync server (single binary / Docker). |
| `tests/` | QEMU boot tests and end-to-end sync tests. |

## Build and boot the image

Everything runs in the dev container; the host needs only Docker.

```sh
dev/run.sh --root make image      # build out/image/stylos.img.raw
dev/run.sh --root make test-boot  # boot UEFI + BIOS, write out/screenshot-*.png
```

`make test-boot` uses the image from `make image` and checks the WP1 gate
(serial console + screenshots).  See `image/README.md` for the image layout
and `tests/boot/run.py` for the assertions.

## License

GPL-3.0-or-later for the OS, shell and daemon. MIT for `protocol/` and
`server/`, so that other services (such as Nostos) can implement the protocol.
