#!/usr/bin/env bash
# Run a command inside the Carrel dev container with the repo at /work.
# Usage: dev/run.sh [--root] <command...>   (no args: interactive shell)
# Runs as your host user so files stay yours. Use --root (privileged) for
# image builds and QEMU/loop-device work; build outputs are handed back to
# your user by the container entrypoint.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
image="carrel-dev:$(cat "$root/dev/Dockerfile" "$root/dev/entrypoint.sh" | sha256sum | cut -c1-12)"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build -t "$image" "$root/dev" >&2
fi
# Release keys live outside git but both `make keys` and `make image` run in
# the container.  Mount the host's Carrel config directory so the private keys
# are written back to (and read from) ~/.config/carrel/keys.
carrel_config="${CARREL_CONFIG_DIR:-${HOME:-/home/dev}/.config/carrel}"
mkdir -p "$carrel_config"
args=(--rm --init -v "$root:/work" -w /work
      -v "$carrel_config:/carrel-config"
      -v carrel-cargo-registry:/opt/cargo/registry
      -v carrel-cargo-git:/opt/cargo/git
      # mkosi mounts an overlayfs over the image during build-package
      # installation. Docker's own overlay2 storage cannot hold an overlay
      # upperdir, so give /var/tmp (mkosi's workspace) a real filesystem.
      -v carrel-dev-tmp:/var/tmp
      -e CARGO_TARGET_DIR=/work/target
      -e CARREL_KEY_DIR=/carrel-config/keys
      -e "CARREL_UID=$(id -u)" -e "CARREL_GID=$(id -g)")
[ -e /dev/kvm ] && args+=(--device /dev/kvm)
[ -t 0 ] && args+=(-it)
if [ "${1:-}" = "--root" ]; then
  shift
  args+=(--privileged -e CARREL_CHOWN=1)   # mkosi needs loop devices and namespaces
else
  args+=(--user "$(id -u):$(id -g)" -e HOME=/tmp -e CARREL_CHOWN=0)
fi
if [ $# -eq 0 ]; then set -- bash; fi
# The Google Drive client IDs live outside git.  Put them at a fixed path in
# the repository for the duration of the container run (0600, gitignored) and
# remove the copy afterwards.  mkosi.build copies the file only when it
# exists; a build without it is a valid self-build image.  The secret is
# never committed and never enters the image unless present here.
oauth="${CARREL_OAUTH_TOML:-${HOME:-/home/dev}/.config/carrel/oauth.toml}"
oauth_copy="$root/.carrel-oauth.toml"
oauth_image_copy="$root/image/.carrel-oauth.toml"
cleanup_oauth=0
if [ -f "$oauth" ]; then
  install -m 0600 "$oauth" "$oauth_copy"
  install -m 0600 "$oauth" "$oauth_image_copy"
  args+=(-e "CARREL_OAUTH_TOML=/work/.carrel-oauth.toml")
  cleanup_oauth=1
fi
status=0
docker run "${args[@]}" "$image" "$@" || status=$?
if [ "$cleanup_oauth" = 1 ]; then
  rm -f "$oauth_copy" "$oauth_image_copy"
fi
exit "$status"
