# Phases and work packages

Each work package (WP) is one branch and one author. A WP is done only when
its **gate** passes inside the dev container (`dev/run.sh …`) and the result
is committed on its branch. A maintainer reviews and merges to `main`.

## Phase 1 — Foundations (parallel)

### WP1 · Dev container + bootable image skeleton (`image`)
- Verify and finish `dev/Dockerfile` + `dev/run.sh`.
- `image/`: mkosi config for Debian 13 amd64 with non-free-firmware, systemd-boot
  UEFI + GRUB BIOS, Cage, a `writer` user, `carrel-session.service` running
  `cage -- <client>` (no `-s`), masked getty, `HandleLidSwitch=suspend`, rfkill block
  wifi at boot, NetworkManager installed, pandoc + typst installed.
- Until `carrel-shell` exists, the session client is a placeholder
  (`/usr/local/bin/carrel-shell` shell script that runs `foot` is NOT allowed —
  no terminals; use a tiny GTK4 "Hello, Carrel" binary or `gtk4-demo`).
- `tests/boot/`: Python QEMU harness (UEFI via OVMF and BIOS) using QMP:
  boot the image, wait for the session, take a screenshot, assert the
  compositor is running and no getty is on tty1-6.
- **Gate:** `dev/run.sh make image && dev/run.sh make test-boot` passes for
  UEFI and BIOS; screenshots saved under `out/`.

### WP2 · Editor core (`shell`)
- Cargo workspace member `shell/` (`carrel-shell`): GTK4 + GtkSourceView 5.
- Editor screen per `spec/03-ui.md` (inline Markdown styling, centered column,
  typewriter scrolling, bottom line, themes Paper/Night, three fonts, font size).
- Durability per `spec/01-architecture.md`: journal with fdatasync ≤1 s,
  atomic save after 5 s idle, replay on start. Put this in a pure-Rust module
  (`shell/src/store/`) with no GTK dependency.
- Files screen (list, new, rename, delete with confirmation), Ctrl+N/Ctrl+O/Esc.
- Writing root from env `CARREL_WRITING_DIR` (default `/data/writing`).
- **Gate:** `dev/run.sh cargo test -p carrel-shell` incl. a crash test that
  spawns a writer process, kills it with SIGKILL at random points 200 times and
  asserts every byte acknowledged >1 s before the kill is recovered;
  `dev/run.sh cargo build --release -p carrel-shell`; a headless screenshot of
  the editor (Cage or `GDK_BACKEND=broadway`/weston headless) in `out/`.

### WP3 · Sync protocol, server and client (`sync`)
- `protocol/` (`carrel-protocol`, MIT): types for `spec/02-sync-protocol.md`.
- `server/` (`carrel-server`, MIT): axum, files on disk as Markdown + a SQLite
  index (revs, file_ids, cursor), pairing with web approval page, mDNS
  announce, TLS with self-signed cert generated on first run, Dockerfile.
- `carreld/` (`carreld`, GPL): sync client module implementing the client
  algorithm and conflict copies (no D-Bus yet; a library + CLI
  `carreld sync --once` for tests).
- **Gate:** `dev/run.sh cargo test -p carrel-protocol -p carrel-server -p carreld`
  with end-to-end tests: pair, push, pull, two clients with a deliberate
  conflict (both versions kept), delete, resume after interrupted sync.

## Phase 2 — The device works

- **WP4 · Daemon + D-Bus** — `carreld` service: D-Bus API, local Git history
  (libgit2), radio control, Wi-Fi via NetworkManager, Tailscale on/off,
  UPower battery (low-battery suspend at 3 %), logind inhibitor. Gate: integration tests in a systemd container.
- **WP5 · Shell screens** — History, Sync, Settings, Wi-Fi, Battery, First run, Export/Import,
  wired to D-Bus. Gate: scripted UI tests + screenshots.
- **WP6 · Export** — pandoc/typst pipeline + Lua citation filter, USB mount
  rules. Gate: export fixture docs to all formats; validate outputs.
- **WP6b · Google Drive** — Google Drive one-way mirror (device flow) in
  `carreld`, per `spec/02-sync-protocol.md`. Gate: tests against a mocked
  Drive API; one manual run against a real account.
- **WP7 · Image integration** — install real binaries into the image; A/B
  partitions, sysupdate, boot counting, recovery menu, installer with "type
  ERASE". Gate: QEMU tests: install from USB image to a blank disk, power-cut
  test (QMP `system_reset` while typing via `sendkey`), update + forced
  rollback.

## Phase 3 — Nostos

- **WP8 · Nostos Cloud side** (in the Nostos repos, not here): implement the
  sync protocol on Nostos Cloud mapping files ↔ Writing Studio drafts, pairing
  page `app.nostos.page/pair`, `/library` export.
- **WP9 · Quotes on the device** — library cache (SQLite FTS5), Quotes palette,
  citation insert. Gate: insert 5 quotes in a scripted test < copy-paste time.

## Phase 4 — Release

- **WP7c · Secure Boot on by default** — a user never changes firmware
  settings. Boot through Debian's Microsoft-signed `shim-signed`, Debian's
  signed systemd-boot and Debian's signed kernel (Type #1 boot entries instead
  of self-signed UKIs; dm-verity root hash stays on the command line). After
  install, Carrel makes itself the default EFI boot entry. Gate: QEMU with OVMF
  and Microsoft keys enrolled (`OVMF_VARS.ms.fd`, Secure Boot enforcing) boots
  the USB image, installs, boots the installed disk, updates and rolls back.
- **WP7d · Release file and docs** — shrink the image to its content and ship
  `carrel-<version>.img.xz` (+ `.sha256`, signature); add `intel-microcode`,
  `amd64-microcode`, `firmware-amd-graphics`; README states that Docker is only
  the build environment and that users flash the image (Fedora Media Writer,
  Etcher, Rufus in DD mode, `dd`). Gate: the compressed image flashes to a
  virtual USB disk and passes the boot tests.
- **WP10 · Hardware** — tier 1 quirks, real-laptop checklist
  (`spec/04-hardware.md`), signed release pipeline, docs site. First real
  laptop: ThinkPad T14 Gen 1 (Intel i5-10210U, UHD graphics, AX201 Wi-Fi,
  Secure Boot on) via "Try Carrel" from USB, internal disk untouched. Gate: all
  tier 1 models pass; 1.0.

## Phase 5 — The writer's screen

- **WP11 · Research** — a literature review on what helps a writer at the
  screen and around it: HCI studies of writing tools, typography and reading
  research, attention, flow and distraction, the writing session as a ritual
  (start, stop, resume), and what writers report about dedicated devices and
  minimal editors. Output: `spec/06-writer-research.md` with findings, sources
  and concrete design rules, each marked by strength of evidence.
- **WP12 · UI review and iteration** — review every screen against
  `spec/00-rules.md` and the research: effortless, non-intrusive, helpful, and
  not a modern tech-company app. Change what does not fit; screenshot before
  and after. Gate: a written review with every rule checked and the changes
  merged.
