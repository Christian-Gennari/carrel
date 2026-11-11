# Phases and work packages

Each work package (WP) is one branch and one worker. A WP is done only when
its **gate** passes inside the dev container (`dev/run.sh …`) and the result
is committed on its branch. The orchestrator reviews and merges to `main`.

## Phase 1 — Foundations (parallel)

### WP1 · Dev container + bootable image skeleton (`image`)
- Verify and finish `dev/Dockerfile` + `dev/run.sh`.
- `image/`: mkosi config for Debian 13 amd64 with non-free-firmware, systemd-boot
  UEFI + GRUB BIOS, Cage, a `writer` user, `stylos-session.service` running
  `cage -s -- <client>`, masked getty, `HandleLidSwitch=suspend`, rfkill block
  wifi at boot, NetworkManager installed, pandoc + typst installed.
- Until `stylos-shell` exists, the session client is a placeholder
  (`/usr/local/bin/stylos-shell` shell script that runs `foot` is NOT allowed —
  no terminals; use a tiny GTK4 "Hello, Stylos" binary or `gtk4-demo`).
- `tests/boot/`: Python QEMU harness (UEFI via OVMF and BIOS) using QMP:
  boot the image, wait for the session, take a screenshot, assert the
  compositor is running and no getty is on tty1-6.
- **Gate:** `dev/run.sh make image && dev/run.sh make test-boot` passes for
  UEFI and BIOS; screenshots saved under `out/`.

### WP2 · Editor core (`shell`)
- Cargo workspace member `shell/` (`stylos-shell`): GTK4 + GtkSourceView 5.
- Editor screen per `spec/03-ui.md` (inline Markdown styling, centered column,
  typewriter scrolling, bottom line, themes Paper/Night, three fonts, font size).
- Durability per `spec/01-architecture.md`: journal with fdatasync ≤1 s,
  atomic save after 5 s idle, replay on start. Put this in a pure-Rust module
  (`shell/src/store/`) with no GTK dependency.
- Files screen (list, new, rename, delete with confirmation), Ctrl+N/Ctrl+O/Esc.
- Writing root from env `STYLOS_WRITING_DIR` (default `/data/writing`).
- **Gate:** `dev/run.sh cargo test -p stylos-shell` incl. a crash test that
  spawns a writer process, kills it with SIGKILL at random points 200 times and
  asserts every byte acknowledged >1 s before the kill is recovered;
  `dev/run.sh cargo build --release -p stylos-shell`; a headless screenshot of
  the editor (Cage or `GDK_BACKEND=broadway`/weston headless) in `out/`.

### WP3 · Sync protocol, server and client (`sync`)
- `protocol/` (`stylos-protocol`, MIT): types for `spec/02-sync-protocol.md`.
- `server/` (`stylos-server`, MIT): axum, files on disk as Markdown + a SQLite
  index (revs, file_ids, cursor), pairing with web approval page, mDNS
  announce, TLS with self-signed cert generated on first run, Dockerfile.
- `stylosd/` (`stylosd`, GPL): sync client module implementing the client
  algorithm and conflict copies (no D-Bus yet; a library + CLI
  `stylosd sync --once` for tests).
- **Gate:** `dev/run.sh cargo test -p stylos-protocol -p stylos-server -p stylosd`
  with end-to-end tests: pair, push, pull, two clients with a deliberate
  conflict (both versions kept), delete, resume after interrupted sync.

## Phase 2 — The device works

- **WP4 · Daemon + D-Bus** — `stylosd` service: D-Bus API, local Git history
  (libgit2), radio control, Wi-Fi via NetworkManager, Tailscale on/off,
  logind inhibitor. Gate: integration tests in a systemd container.
- **WP5 · Shell screens** — History, Sync, Settings, First run, Export/Import,
  wired to D-Bus. Gate: scripted UI tests + screenshots.
- **WP6 · Export** — pandoc/typst pipeline + Lua citation filter, USB mount
  rules. Gate: export fixture docs to all formats; validate outputs.
- **WP7 · Image integration** — install real binaries into the image; A/B
  partitions, sysupdate, boot counting, recovery menu, installer with "type
  ERASE". Gate: QEMU tests: install from USB image to a blank disk, power-cut
  test (QMP `system_reset` while typing via `sendkey`), update + forced
  rollback.

## Phase 3 — Nostos

- **WP8 · Nostos Cloud side** (in the Nostos repos, not here): implement the
  sync protocol on Nostos Cloud mapping files ↔ Writing Studio drafts, pairing
  page `nostos.app/pair`, `/library` export.
- **WP9 · Quotes on the device** — library cache (SQLite FTS5), Quotes palette,
  citation insert. Gate: insert 5 quotes in a scripted test < copy-paste time.

## Phase 4 — Release

- **WP10 · Hardware** — tier 1 quirks, real-laptop checklist
  (`spec/04-hardware.md`), signed release pipeline, docs site. Gate: all tier 1
  models pass; 1.0.
