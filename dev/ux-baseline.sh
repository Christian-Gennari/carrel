#!/usr/bin/env bash
# Run the GTK keystroke baseline under Xvfb and refresh docs/ux-baseline.md.
#
#   dev/run.sh bash dev/ux-baseline.sh
#
# `cargo test` without a display skips the GTK scenarios, so this starts a
# throwaway Xvfb on :98. The test prints the key table and writes the doc.
set -euo pipefail
cd "$(dirname "$0")/.."

mkdir -p out
export XDG_RUNTIME_DIR=/tmp/carrel-ux
rm -rf "$XDG_RUNTIME_DIR"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export DISPLAY=:98
export NO_AT_BRIDGE=1
export GTK_A11Y=none
export GSK_RENDERER=cairo
export GDK_BACKEND=x11

Xvfb :98 -screen 0 1280x800x24 -nolisten tcp >out/ux-xvfb.log 2>&1 &
xvfb_pid=$!
cleanup() {
    kill "$xvfb_pid" 2>/dev/null || true
    wait "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT
sleep 1

cargo test -p carrel-shell --lib gtk_scenarios -- --nocapture
