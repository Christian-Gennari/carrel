# AGENTS.md — Stylos

Read `spec/00-rules.md` first. Then the spec for your work package in
`spec/05-phases.md` and the specs it points to. The specs win over your own
ideas; if a spec is wrong or impossible, say so in your final summary and
choose the smallest change that keeps the rules.

## Environment

- The host has **no sudo** and no Rust/GTK/mkosi/QEMU. Do every build and test
  inside the dev container: `dev/run.sh <command>` (Debian 13, repo mounted at
  `/work`). Add packages to `dev/Dockerfile`, never to the host.
- This machine is shared. Run heavy commands through the load guard:
  `lg run -- dev/run.sh <command>`. If `lg` says "queued", wait. Exit code 75:
  wait 2 minutes and retry (max 2 times).
- Never leave containers, servers or watchers running when you finish.

## Code

- Rust stable, edition 2021, one Cargo workspace at the repo root. If you add
  a crate, add it to `[workspace] members` in `Cargo.toml`.
- `cargo fmt` and `cargo clippy -- -D warnings` must pass for your crates.
- Licenses: `protocol/` and `server/` MIT; everything else GPL-3.0-or-later.
  Put `license = "..."` in each `Cargo.toml`.
- Keep pure logic (storage, sync, protocol) free of GTK so it is testable.
- No network access from `stylos-shell`, ever.

## Git

- Work on your own branch/worktree. Small commits, imperative messages.
- Do not push, do not merge to `main`, do not rewrite `main`. The orchestrator
  merges.
- Commit build outputs never (`out/`, `target/`, images are gitignored).

## Done

Your work package is done when its **Gate** in `spec/05-phases.md` passes.
Your final message: what you built, the exact gate commands you ran and their
result, and anything in the spec you had to change and why.
