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
# The Google Drive client IDs live outside git.  Put them at a fixed path in
# the repository for the duration of the container run (0600, gitignored) and
# remove the copy afterwards.  mkosi.build copies the file only when it
# exists; a build without it is a valid self-build image.  The secret is
# never committed and never enters the image unless present here.
oauth="${STYLOS_OAUTH_TOML:-${HOME:-/home/dev}/.config/stylos/oauth.toml}"
oauth_copy="$root/.stylos-oauth.toml"
oauth_image_copy="$root/image/.stylos-oauth.toml"
cleanup_oauth=0
if [ -f "$oauth" ]; then
  install -m 0600 "$oauth" "$oauth_copy"
  install -m 0600 "$oauth" "$oauth_image_copy"
  args+=(-e "STYLOS_OAUTH_TOML=/work/.stylos-oauth.toml")
  cleanup_oauth=1
fi
status=0
docker run "${args[@]}" "$image" "$@" || status=$?
if [ "$cleanup_oauth" = 1 ]; then
  rm -f "$oauth_copy" "$oauth_image_copy"
fi
exit "$status"
