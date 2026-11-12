# AGENTS.md — Carrel

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
- No network access from `carrel-shell`, ever.

## Git

- Remote: private GitHub repo `Christian-Gennari/carrel`. `main` changes only
  through merged pull requests; never push to `main` or rewrite it.
- Work on your own branch/worktree. Small commits, imperative messages.
- Every PR closes an issue. If your work package has no issue, create one
  (`gh issue create`, with labels from `gh label list`).
- When your gate passes: push your branch and open a PR to `main` with
  `gh pr create` (title = work package, body = your final summary plus
  `Closes #N`). Do not merge it; the orchestrator reviews and merges. Merged
  PRs let T3 remove the worktree automatically.
- Commit build outputs never (`out/`, `target/`, images are gitignored).

## Done

Your work package is done when its **Gate** in `spec/05-phases.md` passes.
Your final message: what you built, the exact gate commands you ran and their
result, and anything in the spec you had to change and why.
