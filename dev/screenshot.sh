#!/usr/bin/env bash
# Headless screenshots of every screen and state for the UI polish rounds.
#
# Runs inside the dev container after `cargo build --release -p carrel-shell`:
#   dev/run.sh bash dev/screenshot.sh
#
# The whole set runs at the two review sizes, into out/shots/<size>/, and
# out/shots/index.html is a contact sheet grouped by screen.
#
#   CARREL_SHOT_SIZES="1366x768 1920x1080"   the sizes to shoot (default)
#   CARREL_SHOT_SIZE=1440x900                one size instead (optional)
#   CARREL_SHOT_THEME_A / CARREL_SHOT_THEME_B   theme ids (default light/study)
#
# Theme ids are deliberately just parameters: the theme round renames them
# without touching a shot. Use lowercase slugs, because they become file names;
# the editor and Settings are shot in all six themes in `shoot_all`.
#
# Xvfb is already part of dev/Dockerfile; no GPU, compositor or display is
# needed. The shell runs with CARREL_FAKE_DAEMON=1, so every screen has
# deterministic data and the daemon is never required. A state that only the
# keyboard reaches is named as CARREL_SHOT_KEYS ("Down,Down,Return").
set -euo pipefail
cd "$(dirname "$0")/.."

theme_a=${CARREL_SHOT_THEME_A:-light}
theme_b=${CARREL_SHOT_THEME_B:-study}
if [ -n "${CARREL_SHOT_SIZE:-}" ]; then
    sizes=${CARREL_SHOT_SIZES:-$CARREL_SHOT_SIZE}
else
    sizes=${CARREL_SHOT_SIZES:-"1366x768 1920x1080"}
fi

out="$PWD/out"
shots="$out/shots"
root="$out/screenshot-data"
rm -rf "$shots" "$root"
mkdir -p "$shots/logs" "$root/.carrel" "$root/Novel"

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
    "day_theme": "light",
    "night_theme": "dark",
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

display=:99
xvfb_pid=
app_pid=
size=
cleanup() {
    if [ -n "${app_pid:-}" ]; then
        kill "$app_pid" 2>/dev/null || true
        wait "$app_pid" 2>/dev/null || true
    fi
    if [ -n "${xvfb_pid:-}" ]; then
        kill "$xvfb_pid" 2>/dev/null || true
        wait "$xvfb_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

start_display() {
    Xvfb "$display" -screen 0 "${size}x24" -nolisten tcp \
        >"$shots/logs/xvfb-$size.log" 2>&1 &
    xvfb_pid=$!
    sleep 1
}

stop_display() {
    kill "$xvfb_pid" 2>/dev/null || true
    wait "$xvfb_pid" 2>/dev/null || true
    xvfb_pid=
}

# shoot <name> <theme> <first-run true|false> <bottom-line true|false> <start> <battery> [extra env...]
# CARREL_SHOT_KEYS, CARREL_SHOT_FOCUS and CARREL_SHOT_GOAL come from the
# caller's environment, like the extra assignments below.
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
    rm -f "$root"/Untitled*.md "$root"/Chapter\ two.md
    env CARREL_WRITING_DIR="$root" DISPLAY="$display" \
        CARREL_FAKE_DAEMON=1 CARREL_START_SCREEN="$start" CARREL_BATTERY="$battery" \
        CARREL_WINDOW_SIZE="$size" \
        "$@" ./target/release/carrel-shell \
        >"$shots/logs/app-$size-$name.log" 2>&1 &
    app_pid=$!
    sleep 2.5
    if ! kill -0 "$app_pid" 2>/dev/null; then
        echo "carrel-shell exited before $name at $size; see out/shots/logs/app-$size-$name.log" >&2
        cat "$shots/logs/app-$size-$name.log" >&2
        exit 1
    fi
    DISPLAY="$display" import -window root "$shots/$size/$name.png"
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
    app_pid=
}

shoot_all() {
    echo "--- $size ---"
    mkdir -p "$shots/$size"

    # The editor in every theme, then the bottom line toggled with F1, focus
    # mode, the hairline goal and an empty new file (Ctrl+N).
    for slug in light sepia mist study dark high-contrast; do
        shoot editor-$slug "$slug" true true editor ""
    done
    CARREL_SHOT_KEYS="F1" shoot editor-$theme_a-bottom-off "$theme_a" true true editor ""
    CARREL_SHOT_FOCUS=true shoot editor-$theme_a-focus "$theme_a" true true editor ""
    CARREL_SHOT_GOAL=hairline shoot editor-$theme_a-hairline "$theme_a" true true editor ""
    CARREL_SHOT_GOAL=hairline shoot editor-$theme_b-hairline "$theme_b" true true editor ""
    CARREL_SHOT_KEYS="ctrl+n" shoot editor-$theme_a-empty "$theme_a" true true editor ""

    # Files: the list, the rename bar (F2) and the delete confirmation (Del).
    shoot files-list-$theme_a "$theme_a" true true files ""
    CARREL_SHOT_KEYS="Down,Down,F2" shoot files-rename-$theme_a "$theme_a" true true files ""
    CARREL_SHOT_KEYS="Down,Down,Delete" shoot files-delete-$theme_a "$theme_a" true true files ""

    # History: the list with its preview, another version previewed, and the
    # restore confirmation.
    shoot history-list-$theme_a "$theme_a" true true history ""
    CARREL_SHOT_KEYS="Down" shoot history-preview-$theme_a "$theme_a" true true history ""
    CARREL_SHOT_KEYS="Return" shoot history-restore-$theme_a "$theme_a" true true history ""

    # Quotes palette: typing `@` in the editor opens it at the caret. The shell
    # types the text itself (CARREL_SHOT_QUOTE_TEXT), in both themes, then the
    # empty states.
    q=$'\n\nNothing in the library says it better: @attention'
    shoot quotes-$theme_a "$theme_a" true true editor "" CARREL_SHOT_QUOTE_TEXT="$q"
    shoot quotes-$theme_b "$theme_b" true true editor "" CARREL_SHOT_QUOTE_TEXT="$q"
    shoot quotes-bare-$theme_a "$theme_a" true true editor "" CARREL_SHOT_QUOTE_TEXT=$'\n\n@'
    shoot quotes-empty-$theme_a "$theme_a" true true editor "" CARREL_FAKE_LIBRARY=empty CARREL_SHOT_QUOTE_TEXT=$'\n\n@'
    shoot quotes-unavailable-$theme_a "$theme_a" true true editor "" CARREL_FAKE_LIBRARY=unavailable CARREL_SHOT_QUOTE_TEXT=$'\n\n@'
    shoot quotes-nomatch-$theme_a "$theme_a" true true editor "" CARREL_SHOT_QUOTE_TEXT=$'\n\n@zzzqx'

    # Sync: unpaired, paired idle, syncing, error, then each pairing flow.
    shoot sync-unpaired-$theme_a "$theme_a" true true sync "" CARREL_FAKE_SYNC=unpaired
    shoot sync-idle-$theme_a "$theme_a" true true sync ""
    shoot sync-syncing-$theme_a "$theme_a" true true sync "" CARREL_FAKE_SYNC=syncing
    shoot sync-error-$theme_a "$theme_a" true true sync "" CARREL_FAKE_SYNC=error
    shoot sync-pair-nostos-$theme_a "$theme_a" true true pair-nostos ""
    shoot sync-pair-selfhosted-$theme_a "$theme_a" true true pair-selfhosted ""
    shoot sync-pair-selfhosted-code-$theme_a "$theme_a" true true pair-selfhosted-code ""
    shoot sync-pair-gdrive-$theme_a "$theme_a" true true pair-gdrive ""

    # Wi-Fi: the scan list, the password bar (Down on a locked network, Enter)
    # and the captive-portal error (Down, Down on Hotel Guest, Enter).
    shoot wifi-scan-$theme_a "$theme_a" true true wifi ""
    CARREL_SHOT_KEYS="Down,Return" shoot wifi-password-$theme_a "$theme_a" true true wifi ""
    CARREL_SHOT_KEYS="Down,Down,Return" shoot wifi-error-$theme_a "$theme_a" true true wifi ""

    # Export: each step, the export in progress, the finished .docx, no drive,
    # and the import result.
    shoot export-what-$theme_a "$theme_a" true true export ""
    CARREL_SHOT_KEYS="Down,Return" shoot export-folder-$theme_a "$theme_a" true true export ""
    CARREL_SHOT_KEYS="Return" shoot export-format-$theme_a "$theme_a" true true export ""
    CARREL_SHOT_KEYS="Return,Return" shoot export-drive-$theme_a "$theme_a" true true export ""
    CARREL_SHOT_KEYS="Return,Return,Return" shoot export-running-$theme_a "$theme_a" true true export "" CARREL_FAKE_SLOW_EXPORT=1
    CARREL_SHOT_KEYS="Return,Down,Down,Return,Return" shoot export-docx-done-$theme_a "$theme_a" true true export ""
    shoot export-no-drive-$theme_a "$theme_a" true true export-drive "" CARREL_FAKE_NO_DRIVE=1
    CARREL_SHOT_KEYS="Down,Down,Down,Return,Return" shoot import-$theme_a "$theme_a" true true export ""

    # Settings: every theme, then Automatic with its day and night pickers, the
    # middle rows, the update row and the last row.
    for slug in light sepia mist study dark high-contrast; do
        shoot settings-$slug "$slug" true true settings ""
    done
    shoot settings-automatic automatic true true settings ""
    CARREL_SETTINGS_SELECT=8 shoot settings-middle-$theme_a "$theme_a" true true settings ""
    CARREL_SETTINGS_SELECT=15 shoot update-ready-$theme_a "$theme_a" true true settings ""
    CARREL_SETTINGS_SELECT=99 shoot settings-last-$theme_a "$theme_a" true true settings ""

    # First run: every step.
    shoot first-run-$theme_a-language "$theme_a" false true first-run ""
    CARREL_SHOT_KEYS="Return" shoot first-run-$theme_a-keyboard "$theme_a" false true first-run ""
    CARREL_SHOT_KEYS="Return,Return" shoot first-run-$theme_a-timezone "$theme_a" false true first-run ""
    CARREL_SHOT_KEYS="Return,Return,Return" shoot first-run-$theme_a-sync "$theme_a" false true first-run ""
    shoot first-run-$theme_b-language "$theme_b" false true first-run ""

    # Battery: charging on the bottom line, low on the line, low and critical
    # with the line hidden (the note shows for 5 s at 10 % and 5 %).
    shoot battery-charging-$theme_a "$theme_a" true true editor "18,charging"
    shoot battery-low-line-$theme_a "$theme_a" true true editor "10"
    shoot battery-low-hidden-$theme_a "$theme_a" true false editor "10"
    shoot battery-critical-$theme_a "$theme_a" true false editor "5"
}

for size in $sizes; do
    case "$size" in
        [0-9]*x[0-9]*) ;;
        *) echo "bad screen size: $size" >&2; exit 2 ;;
    esac
    start_display
    shoot_all
    stop_display
done

# Contact sheet: one section per screen, images from every size, file names as
# captions. Relative paths keep it usable from the worktree.
python3 - "$shots" <<'PY'
import html, pathlib, sys

shots = pathlib.Path(sys.argv[1])
screens = [
    "editor", "files", "history", "quotes", "sync", "wifi",
    "export", "import", "settings", "first-run", "battery", "update",
]
preferred = ["1366x768", "1920x1080"]
sizes = [name for name in preferred if (shots / name).is_dir()]
sizes += sorted(
    path.name for path in shots.iterdir()
    if path.is_dir() and path.name != "logs" and path.name not in preferred
)
names = sorted({path.stem for size in sizes for path in (shots / size).glob("*.png")})

def screen_of(name):
    for screen in screens:
        if name == screen or name.startswith(screen + "-"):
            return screen
    return "other"

groups = {}
for name in names:
    groups.setdefault(screen_of(name), []).append(name)

titles = {
    "first-run": "First run", "wifi": "Wi-Fi", "update": "Update",
}
page = []
page.append("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">")
page.append("<title>Carrel screenshots</title>")
page.append(
    "<style>"
    "body{background:#f6f4ee;color:#262523;font-family:system-ui,sans-serif;"
    "margin:28px;line-height:1.4}"
    "h1{font-weight:500;margin:0 0 4px}p.note{color:#6b6862;margin:0 0 18px}"
    "h2{font-weight:500;margin:38px 0 10px;border-top:1px solid #d8d2c6;"
    "padding-top:14px}"
    ".shots{display:grid;grid-template-columns:repeat(auto-fill,minmax(430px,1fr));"
    "gap:16px}"
    "figure{margin:0}img{width:100%;height:auto;border:1px solid #d8d2c6;"
    "display:block}"
    "figcaption{font:12px ui-monospace,monospace;color:#6b6862;margin-top:4px}"
    "</style></head><body>"
)
page.append("<h1>Carrel screenshots</h1>")
page.append(
    f"<p class=\"note\">{len(names)} states at {', '.join(sizes)}. "
    "Generated by dev/screenshot.sh; every image is a full screen capture.</p>"
)
for screen in screens + ["other"]:
    entries = groups.get(screen)
    if not entries:
        continue
    page.append(f"<h2>{html.escape(titles.get(screen, screen.capitalize()))}</h2>")
    page.append("<div class=\"shots\">")
    for name in entries:
        for size in sizes:
            if not (shots / size / f"{name}.png").is_file():
                continue
            caption = html.escape(f"{size} · {name}.png")
            page.append(
                f"<figure><img src=\"{size}/{name}.png\" alt=\"{html.escape(name)}\" "
                f"loading=\"lazy\"><figcaption>{caption}</figcaption></figure>"
            )
    page.append("</div>")
page.append("</body></html>")
(shots / "index.html").write_text("\n".join(page) + "\n", encoding="utf-8")
print(f"contact sheet: {shots / 'index.html'} ({len(names)} states)")
PY

echo "wrote:"
for size in $sizes; do
    echo "  out/shots/$size ($(find "$shots/$size" -name '*.png' | wc -l) images)"
done
echo "  out/shots/index.html"

echo "--- app logs (empty is good) ---"
for log in "$shots"/logs/app-*.log; do
    [ -s "$log" ] && { echo "$log:"; cat "$log"; }
done
true
