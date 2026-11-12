#!/usr/bin/env bash
# Headless screenshots of the editor for the WP2 gate.
#
# Runs inside the dev container after `cargo build --release -p stylos-shell`:
#   dev/run.sh bash dev/screenshot.sh
#
# Xvfb is already part of dev/Dockerfile; no GPU, compositor or display is
# needed. Writes out/editor-paper.png and out/editor-night.png.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$PWD/out"
root="$out/screenshot-data"
rm -rf "$root"
mkdir -p "$root/.stylos"

cat > "$root/The quiet morning.md" <<'EOF'
# The quiet morning

The kettle clicks off at six. I write at the kitchen table while the house is still **asleep**, and the first sentence arrives as if it had been waiting under the floorboards.

> A sentence is a small room. You walk in, close the door, and look around.

Some mornings the words come *slowly*; other mornings they arrive all at once, like rain on a tin roof. I keep a **notebook** beside the plate and write down what the light touches first.[^1](nostos:cite/quiet-mornings-42)
EOF

# Last file and stats are also stored under .stylos; create only settings.
write_settings() {
    python3 - "$root" "$1" <<'PY'
import json, sys
root, theme = sys.argv[1], sys.argv[2]
settings = {
    "theme": theme,
    "font": "serif",
    "font_size": 18,
    "daily_goal": 500,
    "show_bottom_line": True,
    "typewriter_mode": False,
    "spellcheck": False,
    "clock": False,
}
with open(f"{root}/.stylos/settings.json", "w", encoding="utf-8") as handle:
    json.dump(settings, handle)
PY
}

export XDG_RUNTIME_DIR=/tmp/stylos-xdg
rm -rf "$XDG_RUNTIME_DIR"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export NO_AT_BRIDGE=1
export GTK_A11Y=none
export GSK_RENDERER=cairo
export GDK_BACKEND=x11

Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
xvfb_pid=$!
cleanup() {
    if [ -n "${app_pid:-}" ]; then
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    fi
    kill "$xvfb_pid" 2>/dev/null || true
    wait "$xvfb_pid" 2>/dev/null || true
}
trap cleanup EXIT
sleep 1

shoot() {
    theme=$1
    output=$2
    write_settings "$theme"
    STYLOS_WRITING_DIR="$root" DISPLAY=:99 \
        ./target/release/stylos-shell >"$out/app-$theme.log" 2>&1 &
    app_pid=$!
    sleep 3
    if ! kill -0 "$app_pid" 2>/dev/null; then
        echo "stylos-shell exited before the screenshot; see out/app-$theme.log" >&2
        cat "$out/app-$theme.log" >&2
        exit 1
    fi
    DISPLAY=:99 import -window root "$output"
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=
}

shoot paper "$out/editor-paper.png"
shoot night "$out/editor-night.png"

echo "wrote out/editor-paper.png and out/editor-night.png"
