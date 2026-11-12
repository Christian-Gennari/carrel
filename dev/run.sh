#!/usr/bin/env bash
# Run a command inside the Stylos dev container with the repo at /work.
# Usage: dev/run.sh [--root] <command...>   (no args: interactive shell)
# Runs as your host user so files stay yours. Use --root (privileged) for
# image builds and QEMU/loop-device work; build outputs are handed back to
# your user by the container entrypoint.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
image="stylos-dev:$(cat "$root/dev/Dockerfile" "$root/dev/entrypoint.sh" | sha256sum | cut -c1-12)"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -t "$image" "$root/dev" >&2
fi
args=(--rm --init -v "$root:/work" -w /work
      -v stylos-cargo-registry:/opt/cargo/registry
      -v stylos-cargo-git:/opt/cargo/git
      # mkosi mounts an overlayfs over the image during build-package
      # installation. Docker's own overlay2 storage cannot hold an overlay
      # upperdir, so give /var/tmp (mkosi's workspace) a real filesystem.
      -v stylos-dev-tmp:/var/tmp
      -e CARGO_TARGET_DIR=/work/target
      -e "STYLOS_UID=$(id -u)" -e "STYLOS_GID=$(id -g)")
[ -e /dev/kvm ] && args+=(--device /dev/kvm)
[ -t 0 ] && args+=(-it)
if [ "${1:-}" = "--root" ]; then
  shift
  args+=(--privileged -e STYLOS_CHOWN=1)   # mkosi needs loop devices and namespaces
else
  args+=(--user "$(id -u):$(id -g)" -e HOME=/tmp -e STYLOS_CHOWN=0)
fi
if [ $# -eq 0 ]; then set -- bash; fi
exec docker run "${args[@]}" "$image" "$@"
