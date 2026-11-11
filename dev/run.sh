#!/usr/bin/env bash
# Run a command inside the Stylos dev container with the repo at /work.
# Usage: dev/run.sh [--root] <command...>   (no args: interactive shell)
# Runs as your host user so files stay yours. Use --root (privileged) only
# for image builds and QEMU/loop-device work.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
image="stylos-dev:$(sha256sum "$root/dev/Dockerfile" | cut -c1-12)"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -t "$image" "$root/dev" >&2
fi
args=(--rm -v "$root:/work" -w /work
      -v stylos-cargo-registry:/opt/cargo/registry
      -e CARGO_TARGET_DIR=/work/target)
[ -e /dev/kvm ] && args+=(--device /dev/kvm)
[ -t 0 ] && args+=(-it)
if [ "${1:-}" = "--root" ]; then
  shift
  args+=(--privileged)   # mkosi needs loop devices and namespaces
else
  args+=(--user "$(id -u):$(id -g)" -e HOME=/tmp)
fi
if [ $# -eq 0 ]; then set -- bash; fi
exec docker run "${args[@]}" "$image" "$@"
