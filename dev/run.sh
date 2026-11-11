#!/usr/bin/env bash
# Run a command inside the Stylos dev container with the repo at /work.
# Usage: dev/run.sh <command...>     (no args: interactive shell)
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
# mkosi needs privileges for loop devices and namespaces.
args+=(--privileged)
if [ $# -eq 0 ]; then set -- bash; fi
exec docker run "${args[@]}" "$image" "$@"
