#!/usr/bin/env bash
# Headless screenshots of every screen for the WP5 gate.
#
# Runs inside the dev container after `cargo build --release -p carrel-shell`:
#   dev/run.sh bash dev/screenshot.sh
#
# Xvfb is already part of dev/Dockerfile; no GPU, compositor or display is
# needed. The shell runs with CARREL_FAKE_DAEMON=1, so every screen has
# deterministic data and the daemon is never required.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$PWD/out"
root="$out/screenshot-data"
if [ ! -x target/release/carrel-shell ]; then
    cargo build --release -p carrel-shell
fi
rm -rf "$root"
mkdir -p "$root/.carrel" "$root/Novel"

cat > "$root/The quiet morning.md" <<'EOF'
# The quiet morning

The kettle clicks off at six. I write at the kitchen table while the house is still **asleep**, and the first sentence arrives as if it had been waiting under the floorboards.

> A sentence is a small room. You walk in, close the door, and look around.

Some mornings the words come *slowly*.[^1](nostos:cite/quiet-mornings-42)

As the library puts it, [a sentence is a small room](nostos:cite/small-room-7) and the door closes behind you.[^2](nostos:cite/small-room-7)
EOF

cat > "$root/Novel/Chapter one.md" <<'EOF'
# Chapter one

The train left before the light did.
EOF

cat > "$root/Notes.md" <<'EOF'
# Notes

- buy ink
- fix the back gate
EOF

# Open the sample essay, not whichever file was written last.
cat > "$root/.carrel/state.json" <<'EOF'
{"last_file": "The quiet morning.md"}
EOF

# Every shot starts from this essay: a shot that types into the editor leaves
# an unsaved journal behind, and the next shot would recover it.
cp "$root/The quiet morning.md" "$out/essay.orig"

# Today's words for the bottom line and the hairline: 312 / 500.
printf '{"date": "%s", "words": 312}\n' "$(date +%F)" > "$root/.carrel/stats.json"

write_settings() {
    python3 - "$root" "$1" "$2" "$3" \
        "${CARREL_SHOT_FOCUS:-false}" "${CARREL_SHOT_GOAL:-numbers}" <<'PY'
import json, sys
root, theme, first_run, bottom_line, focus, goal = sys.argv[1:7]
settings = {
    "theme": theme,
    "font": "serif",
    "font_size": 19,
    "daily_goal": 500,
    "goal_display": goal,
    "focus_mode": focus == "true",
    "show_bottom_line": bottom_line == "true",
    "typewriter_mode": False,
    "spellcheck": False,
    "clock": False,
    "language": "English",
    "keyboard": "us",
    "time_zone": "Local time",
    "sync_at_shutdown": False,
    "first_run_complete": first_run == "true",
}
with open(f"{root}/.carrel/settings.json", "w", encoding="utf-8") as handle:
    json.dump(settings, handle)
PY
}

export XDG_RUNTIME_DIR=/tmp/carrel-xdg
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

# shoot <name> <theme> <first-run true|false> <bottom-line true|false> <start> <battery> [extra env...]
shoot() {
    name=$1
    theme=$2
    first_run=$3
    bottom_line=$4
    start=$5
    battery=${6:-}
    shift 6 || true
    write_settings "$theme" "$first_run" "$bottom_line"
    cp "$out/essay.orig" "$root/The quiet morning.md"
    rm -rf "$root/.carrel/journal"
    env CARREL_WRITING_DIR="$root" DISPLAY=:99 \
        CARREL_FAKE_DAEMON=1 CARREL_START_SCREEN="$start" CARREL_BATTERY="$battery" \
        "$@" ./target/release/carrel-shell >"$out/app-$name.log" 2>&1 &
    app_pid=$!
    sleep 2.5
    if ! kill -0 "$app_pid" 2>/dev/null; then
        echo "carrel-shell exited before $name; see out/app-$name.log" >&2
        cat "$out/app-$name.log" >&2
        exit 1
    fi
    DISPLAY=:99 import -window root "$out/$name.png"
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=
}

# The editor, both themes, then focus mode and the hairline goal line.
shoot editor-paper paper true true editor "" 
shoot editor-night night true true editor ""
CARREL_SHOT_FOCUS=true shoot editor-paper-focus paper true true editor ""
CARREL_SHOT_GOAL=hairline shoot editor-night-hairline night true true editor ""

# History with a preview.
shoot history-paper paper true true history ""

# Quotes palette: typing `@` in the editor opens it at the caret. The shell
# types the text itself (CARREL_SHOT_QUOTE_TEXT), in both themes, then the two
# empty states.
q=$'\n\nNothing in the library says it better: @attention'
shoot quotes-paper paper true true editor "" CARREL_SHOT_QUOTE_TEXT="$q"
shoot quotes-night night true true editor "" CARREL_SHOT_QUOTE_TEXT="$q"
shoot quotes-bare-paper paper true true editor "" CARREL_SHOT_QUOTE_TEXT=$'\n\n@'
shoot quotes-empty-paper paper true true editor "" CARREL_FAKE_LIBRARY=empty CARREL_SHOT_QUOTE_TEXT=$'\n\n@'
shoot quotes-unavailable-paper paper true true editor "" CARREL_FAKE_LIBRARY=unavailable CARREL_SHOT_QUOTE_TEXT=$'\n\n@'
shoot quotes-nomatch-paper paper true true editor "" CARREL_SHOT_QUOTE_TEXT=$'\n\n@zzzqx'

# Sync: the overview, then each pairing flow.
shoot sync-paper paper true true sync ""
shoot sync-pair-nostos-paper paper true true pair-nostos ""
shoot sync-pair-selfhosted-paper paper true true pair-selfhosted ""
shoot sync-pair-selfhosted-code-paper paper true true pair-selfhosted-code ""
shoot sync-pair-gdrive-paper paper true true pair-gdrive ""

# Settings, Paper and Night.
shoot settings-paper paper true true settings ""
shoot settings-night night true true settings ""
# The last rows (Update, About) must scroll into view at 800 px.
shoot settings-last-paper paper true true settings "" CARREL_SETTINGS_SELECT=99
shoot settings-last-night night true true settings "" CARREL_SETTINGS_SELECT=99

# Wi-Fi scan list.
shoot wifi-paper paper true true wifi ""

# Export: with a drive, and with none.
shoot export-paper paper true true export ""
shoot export-drive-paper paper true true export-drive ""
shoot export-empty-paper paper true true export-drive "" CARREL_FAKE_NO_DRIVE=1

# First run.
shoot first-run-paper paper false true first-run ""
shoot first-run-night night false true first-run ""

# Battery: the bottom line charging, and the 5 s note with the line hidden.
shoot battery-paper paper true true editor "18,charging"
shoot battery-low-paper paper true false editor "10"

echo "wrote:"
ls -1 "$out"/*.png

echo "--- app logs (empty is good) ---"
for log in "$out"/app-*.log; do
    [ -s "$log" ] && { echo "$log:"; cat "$log"; }
done
true
