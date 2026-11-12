# Architecture

## Overview

```
┌──────────────── laptop ────────────────┐
│ Cage (kiosk Wayland compositor)        │
│   └─ stylos-shell (Rust, GTK4)         │
│        editor · files · settings ·     │
│        history · quotes · sync screen  │
│              │ D-Bus (system bus)      │
│ stylosd (Rust, root, systemd service)  │
│   history (libgit2) · sync client ·    │
│   radio on/off · tailscale on/off ·    │
│   library cache (SQLite FTS5) · update │
│ NetworkManager (radio blocked) ·       │
│ logind (lid → suspend) · sysupdate     │
└────────────────────────────────────────┘
          │ HTTPS (only during sync)
   ┌──────┴───────────┐
   │ Nostos Cloud     │   or   stylos-server (LAN / Tailscale)
   └──────────────────┘
```

## Base system

- Debian 13 (trixie), amd64, with `non-free-firmware`.
- Built with **mkosi** from `image/`. Output: a raw disk image for USB sticks
  (`stylos-<version>.img`) plus sysupdate payloads.
- Partitions (GPT, created by `systemd-repart` at first boot / install):
  - ESP (512 MB, systemd-boot, UKIs).
  - `root-a`, `root-b` (2 GB each, erofs or squashfs, read-only, dm-verity
    signed).
  - `data` (rest of disk, ext4, mounted at `/data`). Optional LUKS2.
- `/etc` overlay and `/var` on `data`. User files at `/data/writing/`.
- Updates: **systemd-sysupdate** downloads signed payloads into the inactive
  root slot. **systemd-boot boot counting** (`+3` tries) falls back to the
  previous slot if the new one does not reach `boot-complete.target`.
- Boot: legacy BIOS laptops are common in tier 1 (X230/T430). Ship both a UEFI
  path (systemd-boot) and a BIOS path (GRUB in a BIOS boot partition) on the
  same image.

## Session

- `getty@tty1` is masked. A systemd service `stylos-session.service` runs
  `cage -- stylos-shell` as user `writer` on seat0 (via `systemd --user`
  lingering or a PAM-less `User=` service with `PAMName=login`).
- VT switching disabled (run `cage` WITHOUT `-s`, which would allow it; also `kernel.sysrq=0`).
- If `stylos-shell` exits, systemd restarts it at once.
- `HandleLidSwitch=suspend`. `stylos-shell` holds a logind delay inhibitor and
  flushes the journal and file on `PrepareForSleep(true)`.

## stylos-shell (Rust + GTK4 + GtkSourceView 5)

- Single process. Screens: Editor (default), Files, History, Quotes palette,
  Settings, Sync, First run.
- Markdown editing with GtkSourceView: inline styling (bold looks bold, heading
  markers are dimmed), no split preview.
- Talks to `stylosd` over D-Bus (`org.stylos.Daemon1`) using `zbus`.
- Never touches the network.

## Durability (inside stylos-shell)

1. Every edit is appended to a per-file journal
   `/data/writing/.stylos/journal/<file-id>.log` (operation log: offset,
   delete-length, inserted text, timestamp). `fdatasync` at most 1 s after
   the last write.
2. After 5 s idle (and before suspend, file switch and exit) the full text is
   written atomically: write `name.md.tmp` → `fsync` → `rename` → `fsync` the
   directory. Then the journal is truncated.
3. On start, any journal newer than its file is replayed.
4. `stylosd` commits `/data/writing` to a local Git repo
   (`/data/writing/.stylos/history.git`, libgit2) after 5 min idle, before
   suspend and before each sync. The History screen lists commits per file by
   time; restoring writes the old text as a new edit (never rewrites history).

## stylosd (Rust, tokio, zbus)

- D-Bus API: `Commit()`, `History(path)`, `Restore(path, rev)`, `Sync()`,
  `SyncStatus` signal, `Pair(backend, …)`, `Unpair()`, `WifiScan()`,
  `WifiConnect(…)`, `SearchLibrary(query, filters)`, `UpdateStatus()`,
  `ApplyUpdate()`.
- Radio control: `rfkill block wifi` by default; unblock only during sync,
  pairing or Wi-Fi setup; block again after, even on error.
- Tailscale: `tailscaled` is installed but disabled. During a sync with a
  Tailscale backend: start `tailscaled`, `tailscale up --auth-key=… ` (first
  time) or just start (later), sync, stop.
- Library cache: `/data/.stylos/library.sqlite` with FTS5 over highlights,
  notes, passages; book table for filters.

## Export and import

Writing must always be able to leave the device, with or without a network.

- **USB drive** (always available, no network): `stylosd` mounts removable
  drives only while the Export screen is open (udisks2, `noexec,nosuid,nodev`),
  and unmounts + powers them off when the export finishes.
- **Formats**: Markdown and plain text are copied directly. `.docx`, `.odt`,
  HTML and EPUB are made with **pandoc**; PDF with pandoc + **typst** (small,
  no LaTeX). Both are in the image. A Lua filter turns `nostos:cite/` links
  into footnotes (text from the cached library, or the link label).
- **Sync** (Nostos or self-hosted) is the other export route: the files appear
  as Studio drafts or as plain `.md` files on the user's server.
- **Raw access**: `/data/writing` is plain Markdown on ext4 (unless LUKS), so
  any Linux live USB can read it in an emergency. The recovery menu also has
  "Copy all writing to USB drive".
- D-Bus: `ListDrives()`, `Export(selection, format, drive_id)`,
  `Import(drive_id, paths)`, `ExportProgress` signal.

## Sync

See [02-sync-protocol.md](02-sync-protocol.md).

## Server

`stylos-server` (Rust, axum, single static binary + Docker image) implements
the protocol and stores files as plain Markdown in a folder. Pairing approval
via a minimal web page on the server. Announces itself with mDNS
(`_stylos._tcp`).

## Dev environment

No host packages are required. `dev/Dockerfile` is a Debian 13 image with
Rust (rustup stable), GTK4 + GtkSourceView 5 dev packages, mkosi, QEMU, OVMF,
and test tools. `dev/run.sh <cmd>` runs a command in it with the repo mounted,
`--device /dev/kvm` when present, and cargo caches in a named volume. All
build/test commands in docs and CI go through `dev/run.sh`.
