# User-Facing Action Inventory

Complete inventory of every user-facing shortcut, Settings row, command palette entry, hint-bar entry, and boot/recovery/installer menu action in Carrel (epic #51).

Columns:
- **Action / Key**: the key chord, command label, row label, or menu entry shown to the writer.
- **Defined at**: `file:line` of the handler or declaration.
- **Expected effect**: the visible and persistent behaviour when triggered.
- **Persists**: whether the result persists across a restart (`y` = yes, `n` = session-only, `na` = navigation/dialog/transient).
- **Status**: `ok` or `issue #N` (known epic items #46, #48, #49, #50, or newly filed #64–#69).

---

## 1. Global Shortcuts (`shell/src/keys.rs`)

Active on every screen except First run (`shell/src/keys.rs:112`). Blocked while the Key sheet (`shell/src/keys.rs:109`), Rename sheet (`shell/src/keys.rs:119`), Command palette (`shell/src/keys.rs:125`), or a modal confirmation dialog (`shell/src/keys.rs:131`) is open.

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Ctrl+K` | `shell/src/keys.rs:133` | Open the Command palette over the current screen. | `na` | `ok` |
| `Ctrl+/` | `shell/src/keys.rs:139` | Open the Keyboard shortcuts card over the current screen (`?` stays a normal character in the editor). | `na` | `issue #68` (no keycode fallback on Swedish/non-US layouts) |
| `Ctrl+O` | `shell/src/keys.rs:144`, `shell/src/files.rs:199` | Open the Files screen from any non-first-run screen; on Files, return to the editor. | `na` | `ok` |
| `Ctrl+H` | `shell/src/keys.rs:148` | Open History for the current document. | `na` | `ok` |
| `Ctrl+Shift+S` | `shell/src/keys.rs:152` | Open Sync and start a sync immediately (`SyncPage::sync_now`). | `y` (remote sync) | `ok` |
| `Ctrl+Shift+R` | `shell/src/keys.rs:156` | Open the Rename current file card over the page. | `y` (renames file on disk) | `ok` |
| `Ctrl+E` | `shell/src/keys.rs:160` | Open Export (or return to the Configure rows when already on the Export Done screen). | `na` | `ok` |
| `Ctrl+,` | `shell/src/keys.rs:169` | Open Settings. | `na` | `ok` |

---

## 2. Editor (`shell/src/editor.rs`, `shell/src/quotes.rs`)

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| Plain typing | `shell/src/editor.rs:500` | Insert text, update inline Markdown styling, dim bottom line & title during typing, journal & autosave to `/data/writing/`. | `y` | `ok` (`issue #49` for non-US layouts like Swedish `å ä ö`) |
| `Ctrl+O` | `shell/src/keys.rs:144`, `shell/src/editor.rs:461` | Open the Files screen. | `na` | `ok` |
| `Ctrl+N` | `shell/src/editor.rs:465` | Create a new untitled `.md` document and open it (auto-named from its first line). | `y` | `ok` |
| `Ctrl+=` / `Ctrl++` | `shell/src/editor.rs:471` | Increase editor body font size by 1 px (max 24 px) and save to `settings.json`. | `y` | `issue #50` |
| `Ctrl+-` | `shell/src/editor.rs:475` | Decrease editor body font size by 1 px (min 16 px) and save to `settings.json`. | `y` | `issue #50` |
| `F1` | `shell/src/editor.rs:481` | Toggle the bottom status line (`show_bottom_line`) and save to `settings.json`. | `y` | `ok` |
| `F2` | `shell/src/editor.rs:486` | Toggle Focus mode (`focus_mode`: dims non-current paragraphs to 35% opacity) and save to `settings.json`. | `y` | `ok` |
| `Backspace` / `Delete` (Typewriter mode) | `shell/src/editor.rs:491` | Blocked when `settings.typewriter_mode` is `true`; normal deletion otherwise. | `na` | `ok` |
| `Ctrl+'` | `shell/src/quotes.rs:622` | Insert `@` at caret and open the Quotes palette (`Esc` removes the inserted `@` if nothing was typed). | `na` | `issue #68` (keyval-only, no keycode fallback on non-US layouts) |
| `@` at word start | `shell/src/quote_trigger.rs:60`, `shell/src/quotes.rs:629` | Open the Quotes palette live at the caret (ignored mid-word such as `me@example.com` and inside code spans/fences). | `na` | `ok` |
| `Ctrl+Z` / `Ctrl+Shift+Z` | GTK `sourceview::View` | Standard buffer undo / redo (quote insertion is one atomic undoable step). | `y` (autosaved) | `ok` |

---

## 3. Files Screen (`shell/src/files.rs`)

Note: new documents can appear duplicated between Recent and the folder tree (`issue #48`).

### 3.1 Normal & Search Mode

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Up` / `Down` | `shell/src/files.rs:231-238` | Move selection up / down through visible rows. | `na` | `ok` |
| `Home` / `End` | `shell/src/files.rs:239-246` | Jump to first / last row. | `na` | `ok` |
| `Enter` / `KP_Enter` | `shell/src/files.rs:213` | Open selected document in editor, enter selected folder, go up on `..`, or open conflict resolution dialog on a conflict copy. | `y` (updates `state.json` `last_file`/`recent`) | `ok` (`issue #48` duplicate Recent display) |
| `Esc` | `shell/src/files.rs:205` | Clear active search query if non-empty; otherwise return to the editor. | `na` | `ok` |
| `Backspace` | `shell/src/files.rs:225` | Delete last search character if query non-empty; otherwise go to parent folder. | `na` | `ok` |
| Plain letters / digits | `shell/src/files.rs:254` | Filter documents fuzzy across all folders under `/data/writing`. | `na` | `ok` |
| `F2` | `shell/src/files.rs:221` | Start inline rename of the selected file or folder (extension hidden for documents). | `na` | `ok` (#47 closed by PR #59) |
| `Del` (`Delete`) | `shell/src/files.rs:217` | Open confirmation dialog to move selected file or folder to `.carrel/trash/`. | `y` (on confirm) | `ok` |
| `M` (`Shift+M`) | `shell/src/files.rs:250` | Enter Move mode for the selected document. | `na` | `issue #69` (hijacks `Shift+M` while typing a search query; hint does not indicate `Shift+M`) |
| `Ctrl+N` | `shell/src/files.rs:191-196` | Create a new `.md` file inside the selected/current folder and open it in the editor. | `y` | `ok` |
| `Ctrl+Shift+N` | `shell/src/files.rs:191-193` | Open inline entry to create a new folder in the current directory. | `na` | `ok` |
| `Ctrl+O` | `shell/src/files.rs:199` | Close Files and return to the editor. | `na` | `ok` |

### 3.2 Inline Rename & New Folder Modes

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Enter` / `KP_Enter` | `shell/src/files.rs:173` | Commit rename or create folder on disk. | `y` | `ok` |
| `Esc` | `shell/src/files.rs:169` | Cancel inline edit without changing disk. | `na` | `ok` |
| `Tab` / `Shift+Tab` | `shell/src/files.rs:177` | Consumed so focus stays inside the inline entry. | `na` | `ok` |

### 3.3 Move Mode

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Up` / `Down` / `Home` / `End` | `shell/src/files.rs:278-281` | Move selection among destination folders (including `New folder…`). | `na` | `ok` |
| `Enter` / `KP_Enter` | `shell/src/files.rs:277` | Move document into selected folder (or prompt for a new folder name). | `y` | `ok` |
| `Ctrl+Shift+N` | `shell/src/files.rs:271` | Start inline new-folder prompt directly from Move mode. | `na` | `ok` |
| `Esc` | `shell/src/files.rs:276` | Cancel Move mode and restore the file list. | `na` | `ok` |

### 3.4 Conflict Resolution Card (`shell/src/files.rs:795`, `shell/src/ui.rs:486`)

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Keep mine` | `shell/src/files.rs:802` | Trash the conflict copy, keep the primary file. | `y` | `ok` |
| `Keep theirs` | `shell/src/files.rs:803` | Trash the primary file, rename the conflict copy into its place. | `y` | `ok` |
| `Keep both` | `shell/src/files.rs:804` | Rename the conflict copy to a unique regular name so both remain. | `y` | `ok` |
| `Esc` | `shell/src/ui.rs:500` | Close dialog without modifying either file. | `na` | `ok` |

---

## 4. Command Palette (`shell/src/palette.rs`)

### 4.1 Palette Controls

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Up` / `Down` | `shell/src/palette.rs:494-495` | Move selection (wraps around). | `na` | `ok` |
| `Enter` / `KP_Enter` | `shell/src/palette.rs:496` | Close palette and run selected command or open selected document. | depends on command | `ok` |
| `Esc` / `Ctrl+K` | `shell/src/palette.rs:488,493` | Close the Command palette. | `na` | `ok` |
| `Backspace` | `shell/src/palette.rs:497` | Remove last character of filter query. | `na` | `ok` |
| Printable characters | `shell/src/palette.rs:503` | Append to fuzzy filter query (matches actions and documents under `/data/writing`). | `na` | `ok` |

### 4.2 Every Palette Command (`shell/src/palette.rs:83-171`, `555-662`)

| Command Label | Shortcut Cap | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|---|
| `New file` | `Ctrl+N` | `palette.rs:94,557` | Switch to editor and create a new document. | `y` | `ok` |
| `Open file…` | `Ctrl+O` | `palette.rs:95,563` | Switch to Files screen. | `na` | `ok` |
| `Insert quote` | `Ctrl+'` | `palette.rs:96,565` | Switch to editor and open Quotes palette at caret. | `na` | `ok` |
| `Sync now` | `Ctrl+Shift+S` | `palette.rs:97,570` | Open Sync screen and start a sync. | `y` | `ok` |
| `Export…` | `Ctrl+E` | `palette.rs:98,571` | Switch to Export screen. | `na` | `ok` |
| `History` | `Ctrl+H` | `palette.rs:99,572` | Switch to History screen for current document. | `na` | `ok` |
| `Turn focus mode on` / `Turn focus mode off` | `F2` | `palette.rs:100,573` | Toggle `focus_mode`, save settings, flash status note. | `y` | `ok` |
| `Theme…` | — | `palette.rs:109,585` | Open Settings focused on the Theme row. | `na` | `ok` |
| `Settings` | `Ctrl+,` | `palette.rs:110,586` | Switch to Settings screen. | `na` | `ok` |
| `Keyboard shortcuts` | `Ctrl+/` | `palette.rs:111,587` | Open the Keyboard shortcuts card. | `na` | `ok` |
| `Rename current file…` | `Ctrl+Shift+R` | `palette.rs:114,564` | Open the Rename current file card. | `y` (on save) | `ok` |
| `Larger text` | `Ctrl+=` | `palette.rs:115,588` | Increase editor font size by 1 px (max 24), save, flash size. | `y` | `ok` (command works; `Ctrl+=` key is `issue #50`) |
| `Smaller text` | `Ctrl+-` | `palette.rs:116,593` | Decrease editor font size by 1 px (min 16), save, flash size. | `y` | `ok` (command works; `Ctrl+-` key is `issue #50`) |
| `Back to the editor` | `Esc` | `palette.rs:117,598` | Switch back to the editor and focus it. | `na` | `ok` |
| `Hide bottom line` / `Show bottom line` | `F1` | `palette.rs:118,599` | Toggle `show_bottom_line`, save, flash status note. | `y` | `ok` |
| `Use hairline goal` / `Use word count goal` | — | `palette.rs:127,611` | Switch `goal_display` between `Numbers` and `Hairline`, save, restyle, flash note. | `y` | `ok` |
| `Turn typewriter mode on` / `Turn typewriter mode off` | — | `palette.rs:140,618` | Toggle `typewriter_mode`, save, flash status note. | `y` | `ok` |
| `Theme: Automatic` | — | `palette.rs:149,635` | Set theme to Automatic (day/night by local hour), save, restyle, flash note. | `y` | `ok` (`issue #66` if non-UTC timezone selected) |
| `Theme: Light` | — | `palette.rs:149,635` | Set theme to Light, save, restyle, flash note. | `y` | `ok` |
| `Theme: Sepia` | — | `palette.rs:149,635` | Set theme to Sepia, save, restyle, flash note. | `y` | `ok` |
| `Theme: Mist` | — | `palette.rs:149,635` | Set theme to Mist, save, restyle, flash note. | `y` | `ok` |
| `Theme: Writer's Study` | — | `palette.rs:149,635` | Set theme to Writer's Study, save, restyle, flash note. | `y` | `ok` |
| `Theme: Dark` | — | `palette.rs:149,635` | Set theme to Dark, save, restyle, flash note. | `y` | `ok` |
| `Theme: High contrast` | — | `palette.rs:149,635` | Set theme to High contrast, save, restyle, flash note. | `y` | `ok` |
| `Font: Serif` | — | `palette.rs:156,642` | Set font to Serif (Source Serif 4), save, restyle, flash note. | `y` | `ok` |
| `Font: Sans` | — | `palette.rs:156,642` | Set font to Sans (Inter), save, restyle, flash note. | `y` | `ok` |
| `Font: Mono` | — | `palette.rs:156,642` | Set font to Mono (JetBrains Mono), save, restyle, flash note. | `y` | `ok` |
| `Wi-Fi…` | — | `palette.rs:159,649` | Switch to Wi-Fi screen. | `na` | `ok` |
| `Pair…` | — | `palette.rs:160,650` | Switch to Sync screen. | `na` | `ok` |
| `Install update [<ver>]` | — | `palette.rs:161,651` | Shown only when an update is ready; opens Settings Update row and triggers apply/reboot. | `y` | `ok` |
| `Open: <doc>` (dynamic) | — | `palette.rs:196,655` | Shown when query is non-empty and matches documents; opens document in editor. | `y` (`last_file`) | `ok` |

---

## 5. Key Sheet & Rename Sheet (`shell/src/palette.rs`)

### 5.1 Key Sheet (`shell/src/palette.rs:807-841, 1108`)

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Esc` / `Ctrl+/` | `shell/src/palette.rs:1113` | Close the Keyboard shortcuts card; all other keys are consumed while open. | `na` | `ok` (`issue #68` for `Ctrl+/` on Swedish/non-US layouts) |

Entries displayed on the Key Sheet (`palette.rs:807-832`):
- **Writing**: `Ctrl+K` Everything, `Ctrl+'` Quote, `F1` Bottom line, `F2` Focus mode, `Ctrl+=` Larger text (`issue #50`), `Ctrl+-` Smaller text (`issue #50`), `Ctrl+/` This card (`issue #68`).
- **Files**: `Ctrl+O` Files, `Ctrl+N` New file, `Ctrl+Shift+R` Rename current file, `Enter` Open, `F2` Rename, `Del` Delete.
- **Sync and export**: `Ctrl+Shift+S` Sync now, `Ctrl+E` Export.
- **Screens**: `Ctrl+H` History, `Ctrl+,` Settings, `Esc` Back to the editor.

### 5.2 Rename Sheet (`shell/src/palette.rs:843-1040`)

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Enter` / `KP_Enter` | `shell/src/palette.rs:1032` | Rename current open document on disk, preserve cursor and undo stack, pause auto-naming. | `y` | `ok` |
| `Esc` | `shell/src/palette.rs:1028` | Close card without renaming. | `na` | `ok` |
| `Tab` / `Shift+Tab` | `shell/src/palette.rs:1036` | Consumed so focus stays inside the rename entry. | `na` | `ok` |

---

## 6. Quotes Palette (`shell/src/quotes.rs`)

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Up` / `Down` | `shell/src/quotes.rs:645` | Move selection across matching quotes (when results exist). | `na` | `ok` |
| `Enter` / `KP_Enter` | `shell/src/quotes.rs:655` | Replace `@query` with `> quote` + citation link (one undoable step). | `y` (autosaved) | `ok` |
| `Shift+Enter` / `Ctrl+Enter` | `shell/src/quotes.rs:657` | Replace `@query` with the citation link only (one undoable step). | `y` (autosaved) | `ok` |
| `Esc` | `shell/src/quotes.rs:641` | Close palette; keep typed `@query` (or remove bare `@` if opened via `Ctrl+'` with no query). | `na` | `ok` |
| Typing while open | `shell/src/quotes.rs:629` | Updates live search query after `@`; closes automatically if >80 chars, >8 words, leading space, or double space. | `na` | `ok` |

---

## 7. History Screen (`shell/src/history.rs`)

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Up` / `Down` | `shell/src/history.rs:787-794` | Select previous / next version and load diff preview against current text. | `na` | `ok` |
| `Home` / `End` | `shell/src/history.rs:803-811` | Jump to newest / oldest version. | `na` | `ok` |
| `PgUp` / `PgDn` | `shell/src/history.rs:795-802` | Scroll the diff preview pane up / down. | `na` | `ok` |
| `Enter` / `KP_Enter` | `shell/src/history.rs:812` | Open confirmation dialog to restore selected version (non-destructive; writes a new edit). | `y` (on confirm) | `ok` |
| `Esc` | `shell/src/history.rs:783` | Return to the editor. | `na` | `ok` |

---

## 8. Settings Screen (`shell/src/settings_page.rs`, `shell/src/settings.rs`)

### 8.1 Navigation Keys

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Up` / `Down` | `shell/src/settings_page.rs:586-601` | Move between setting rows (steps over group headings and sample line). | `na` | `ok` |
| `Home` / `End` | `shell/src/settings_page.rs:602-617` | Jump to first / last setting row. | `na` | `ok` |
| `Left` / `Right` | `shell/src/settings_page.rs:618-622` | Change value of selected setting in place and save `.carrel/settings.json`. | `y` | `ok` (per-row status below) |
| `Enter` / `KP_Enter` | `shell/src/settings_page.rs:623-626` | Open sub-screen on `Wi-Fi networks` / `Sync`, or restart & apply on ready `Update`. | `na` | `ok` |
| `Esc` | `shell/src/settings_page.rs:582` | Return to the editor. | `na` | `ok` |

### 8.2 Every Settings Row

| Group | Row Label | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|---|
| *(top, when ready)* | `Update ready (<ver>)` | `settings_page.rs:64,343` | `Enter` applies staged A/B update and reboots into the new slot. | `y` | `ok` |
| Look | `Theme` | `settings_page.rs:226,874` | Cycle `Automatic` / `Light` / `Sepia` / `Mist` / `Writer's Study` / `Dark` / `High contrast`; live restyle. | `y` | `ok` (`issue #66` for Automatic timezone) |
| Look | `Day theme` *(only under Automatic)* | `settings_page.rs:232,875` | Cycle `Light` / `Sepia` / `High contrast` used 07:00–19:00; live restyle. | `y` | `ok` |
| Look | `Night theme` *(only under Automatic)* | `settings_page.rs:238,876` | Cycle `Mist` / `Writer's Study` / `Dark` used 19:00–07:00; live restyle. | `y` | `ok` |
| Look | `Font` | `settings_page.rs:244,877` | Cycle `Serif` / `Sans` / `Mono`; live restyle & sample update. | `y` | `ok` |
| Look | `Size` | `settings_page.rs:257,878` | Step editor body size 16–24 px; live restyle & sample update. | `y` | `ok` |
| Writing | `Daily goal` | `settings_page.rs:266,879` | Step daily word goal 100–2000 by 100 words; updates bottom line & hairline. | `y` | `ok` |
| Writing | `Goal progress` | `settings_page.rs:279,880` | Toggle `Numbers` vs `Hairline`; switches editor word count display vs 2 px bottom hairline. | `y` | `ok` |
| Writing | `Focus mode` | `settings_page.rs:286,881` | Toggle `Off` / `On`; dims non-active paragraphs in editor. | `y` | `ok` |
| Writing | `Typewriter mode` | `settings_page.rs:290,882` | Toggle `Off` / `On`; blocks `Backspace` and `Delete` in editor. | `y` | `ok` |
| Writing | `Spellcheck` | `settings_page.rs:294,883` | Toggle `Off` / `On`; underlines words the `Language` dictionary does not know (`shell/src/speller.rs`), at once. | `y` | `ok` |
| Writing | `Clock` | `settings_page.rs:298,884` | Toggle `Off` / `On`; shows/hides time on the editor bottom line. | `y` | `ok` (`issue #66` for non-UTC timezone) |
| Connections | `Wi-Fi networks` | `settings_page.rs:341,885` | `Enter` opens the Wi-Fi screen. | `na` | `ok` |
| Connections | `Sync` | `settings_page.rs:342,886` | `Enter` opens the Sync screen. | `na` | `ok` |
| Connections | `Sync at shutdown` | `settings_page.rs:302,887` | Toggle `Off` / `On` and save JSON; intended to run a final sync before poweroff. | `y` (JSON only) | `issue #67` (never read at shutdown; no effect) |
| Device | `Keyboard layout` | `settings_page.rs:306,888` | Cycle XKB layout (`us`, `gb`, `de`, `fr`, `es`, `it`, `pt`, `br`, `pl`, `tr`, `se`, `no`, `dk`) and save JSON. | `y` (JSON only) | `issue #49` (never applied to compositor/session) |
| Device | `Language` | `settings_page.rs:312,889` | Cycle UI language (`English`, `Deutsch`, `Français`, …), save JSON and switch the Spellcheck dictionary. | `y` (JSON only) | `issue #65` (UI text is hardcoded English; only Spellcheck follows it) |
| Device | `Time zone` | `settings_page.rs:321,890` | Cycle IANA time zone (`Local time`, `UTC`, `Europe/Stockholm`, …) and save JSON. | `y` (JSON only) | `issue #66` (clock & Automatic theme ignore it) |
| Device | `Update` *(when not ready)* | `settings_page.rs:334,891` | Read-only status (`Up to date`, `Downloading…`, or error). | `na` | `ok` |
| Device | `About` | `settings_page.rs:335,892` | Read-only version row (`Carrel OS <version>`). | `na` | `ok` |

---

## 9. Sync Screen (`shell/src/sync.rs`)

| View | Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|---|
| Overview | `Up` / `Down` | `shell/src/sync.rs:1213-1222` | Move selection across Sync actions (`Sync now`, `Choose a network`, backend choices, `Unpair`). | `na` | `ok` |
| Overview | `Enter` / `KP_Enter` | `shell/src/sync.rs:1223` | Run selected action: sync now, open Wi-Fi handoff, begin pairing (Nostos / Self-hosted / Google Drive), or open Unpair confirmation. | `y` (pair/unpair/sync) | `ok` |
| Overview | `Esc` | `shell/src/sync.rs:1209` | Return to the editor. | `na` | `ok` |
| Self-hosted URL | `Enter` (in URL entry) | `shell/src/sync.rs:647` | Submit server URL and begin pairing (or prompt Wi-Fi if offline, preserving URL). | `na` | `ok` |
| Self-hosted URL | `Esc` | `shell/src/sync.rs:1192` | Return to Sync overview. | `na` | `ok` |
| Pairing | `Esc` | `shell/src/sync.rs:1200` | Cancel pairing and return to Sync overview. | `na` | `ok` |
| Unpair dialog | `Esc` / `Left` / `Right` / `Tab` / `Enter` | `shell/src/sync.rs:1182`, `shell/src/ui.rs:486` | Confirm or cancel unpairing (Cancel focused first). | `y` (on confirm) | `ok` |

---

## 10. Wi-Fi Screen (`shell/src/wifi.rs`)

| Mode | Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|---|
| List | `Up` / `Down` | `shell/src/wifi.rs:548-557` | Move selection through scanned networks and `Other network…`. | `na` | `ok` |
| List | `Enter` / `KP_Enter` | `shell/src/wifi.rs:558` | Connect to open/saved network, open inline password field for secured network, or open hidden-SSID entry for `Other network…`. | `y` (NetworkManager) | `ok` |
| List | `F` | `shell/src/wifi.rs:562` | Open confirmation dialog to forget selected saved network. | `y` (on confirm) | `ok` |
| List | `R` | `shell/src/wifi.rs:569` | Rescan Wi-Fi networks. | `na` | `ok` |
| List | `Esc` | `shell/src/wifi.rs:538` | Return to Sync (if opened from Sync handoff) or editor; re-blocks radio on leave. | `na` | `ok` |
| Password / Other SSID | `Enter` / `KP_Enter` | `shell/src/wifi.rs:531` | Submit password or hidden SSID. | `y` (on connect) | `ok` |
| Password / Other SSID | `Esc` | `shell/src/wifi.rs:519` | Close inline field and return to network list. | `na` | `ok` |
| Forget dialog | `Esc` / `Left` / `Right` / `Tab` / `Enter` | `shell/src/wifi.rs:513`, `shell/src/ui.rs:486` | Confirm or cancel forgetting the network (Cancel focused first). | `y` (on confirm) | `ok` |

---

## 11. Export & Import Screen (`shell/src/export.rs`)

| Step | Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|---|
| Configure | `Up` / `Down` | `shell/src/export.rs:957-966` | Move between `What`, `Format`, and `To` (Export) or `From` (Import). | `na` | `ok` |
| Configure | `Left` / `Right` | `shell/src/export.rs:967-976` | Cycle selection (`Current file` / folder / `Everything`; Markdown / Plain text / Word `.docx` / OpenDocument `.odt` / PDF / HTML / EPUB; USB drive). | `y` (remembered in `.carrel/export-prefs.json` on run) | `ok` |
| Configure | `Enter` / `KP_Enter` | `shell/src/export.rs:977` | Start Export or Import when a USB drive is available (disabled without a drive). | `y` (writes to USB or `/data/writing/Imported/`) | `ok` |
| Configure / Done | `I` | `shell/src/export.rs:985` | Switch to Import mode (`Import from a USB drive`). | `na` | `ok` |
| Configure / Done | `E` | `shell/src/export.rs:991` | Switch to Export mode (`Export instead` / `Export something else`). | `na` | `ok` |
| Done | `Enter` / `KP_Enter` | `shell/src/export.rs:980` | Return to the editor. | `na` | `ok` |
| Any | `Esc` | `shell/src/export.rs:953` | Return to the editor. | `na` | `ok` |

---

## 12. First Run Screen (`shell/src/first_run.rs`)

| Step | Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|---|
| Any | `Up` / `Down` / `Home` / `End` | `shell/src/first_run.rs:314-334` | Move selection across options (`Language`, `Keyboard`, `Time zone`, `Start writing`; or picker list; or sync choices). | `na` | `ok` |
| Welcome | `Enter` on `Language` / `Keyboard` / `Time zone` | `shell/src/first_run.rs:172-182` | Open the respective picker list; choosing an item saves to `settings.json` and returns to Welcome. | `y` | `ok` (settings saved; `issue #49` / `#65` / `#66` for effect) |
| Welcome | `Enter` on `Start writing` | `shell/src/first_run.rs:170` | Advance to `Connect sync?` step (cursor defaults to `Not now`). | `na` | `ok` |
| Connect sync? | `Enter` on `Not now` | `shell/src/first_run.rs:202` | Set `first_run_complete = true`, save `settings.json`, open editor. | `y` | `ok` |
| Connect sync? | `Enter` on `Nostos` / `Self-hosted server` / `Google Drive` | `shell/src/first_run.rs:203-205` | Set `first_run_complete = true`, save `settings.json`, hand off to Sync pairing (or Wi-Fi if offline). | `y` | `ok` |
| Picker / Sync step | `Esc` | `shell/src/first_run.rs:307` | Return one step back to Welcome (no-op on Welcome). | `na` | `ok` |

---

## 13. Shared Confirmation & Choice Dialogs (`shell/src/ui.rs`)

Used by Files delete, Files conflict copy, History restore, Sync unpair, and Wi-Fi forget.

| Action / Key | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `Esc` | `shell/src/ui.rs:500` | Cancel and close dialog without changes. | `na` | `ok` |
| `Left` / `Right` / `Tab` / `Shift+Tab` | `shell/src/ui.rs:510-527` | Move focus between `Cancel` (default) and destructive/choice buttons. | `na` | `ok` |
| `Enter` / `KP_Enter` | `shell/src/ui.rs:501-509` | Activate the currently focused button (`Cancel` by default). | depends on button | `ok` |

---

## 14. Recovery Menu (`image/mkosi.extra/usr/lib/carrel/recovery`)

Booted via `Carrel recovery` (`systemd.unit=carrel-recovery.target`); `whiptail` menu on `tty1`.

| Menu Item | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `previous` — `Boot previous system` | `recovery:22,129` | Confirm and switch `loader.conf` / `grubenv` to inactive slot (`a`/`b`), then reboot. | `y` | `ok` |
| `copy` — `Copy all writing to USB drive` | `recovery:55,130` | List removable/USB drives, mount first partition (FAT or ext4), copy `/data/writing/` into timestamped folder, sync & unmount. | `y` | `ok` (fixed in this PR: previously hardcoded `-t ext4` and `cp -a`, breaking FAT drives) |
| `reset` — `Reset settings (keeps writing)` | `recovery:111,131` | Confirm and remove `/data/writing/.carrel/settings.json` and `stats.json`, keeping writing & history. | `y` | `ok` (fixed in this PR: previously deleted `/data/.carrel/settings.json` instead of `/data/writing/.carrel/settings.json`) |
| `poweroff` — `Power off` (or `Esc`/Cancel) | `recovery:132,139` | Power off the computer (`systemctl poweroff`). | `na` | `ok` |

---

## 15. USB Installer Menu (`image/mkosi.extra/usr/lib/carrel/installer`)

Runs on USB boot (`removable=1` or `TRAN=usb`) before the graphical session.

| Menu Item | Defined at | Expected effect | Persists | Status |
|---|---|---|---|---|
| `try` — `Try Carrel (nothing is changed)` | `installer:95,125` | Exit installer and continue booting into the graphical Carrel session on the USB drive. | `na` | `issue #46` (boot menu wording clarity) |
| `install` — `Install to this computer` | `installer:96,130` | Choose internal disk (>= 8 GB), require typing `ERASE`, `dd` USB image to disk, grow `/data`, register UEFI `Carrel` boot entry, prompt to remove USB & reboot. | `y` | `issue #46` |
| Cancel / `Esc` on main menu | `installer:124` | Power off the computer (`systemctl poweroff`). | `na` | `ok` |

---

## 16. Hint-Bar Inventory

Every hint bar rendered across screens and floating cards (`ui::hint_bar` caps display at 5 items, `shell/src/ui.rs:614`).

| Screen / Card / State | Hint Entries Shown | Defined at | Matches Handlers? | Status |
|---|---|---|---|---|
| Editor (bottom line right edge) | `Ctrl+K` cap | `shell/src/editor.rs:321` | Yes | `ok` |
| Editor (empty doc start hint) | `"Start writing. Ctrl+K for everything else."` | `shell/src/editor.rs:285` | Yes | `ok` |
| Command palette (with rows) | `[Enter] Run` · `[↑ ↓] Move` · `[Esc] Close` | `shell/src/palette.rs:42` | Yes | `ok` |
| Command palette (no matches) | `[Esc] Close` | `shell/src/palette.rs:373` | Yes | `ok` |
| Rename sheet | `[Enter] Save` · `[Esc] Cancel` | `shell/src/palette.rs:843` | Yes | `ok` |
| Key sheet | `[Esc] Close` | `shell/src/palette.rs:1070` | Yes | `ok` |
| Quotes palette (with hits) | `[Enter] Quote` · `[Shift+Enter] Link` · `[Esc] Close` | `shell/src/quotes.rs:39` | Yes | `ok` |
| Quotes palette (message/empty) | `[Esc] Close` | `shell/src/quotes.rs:328` | Yes | `ok` |
| Files (normal) | `[Enter] Open` · `[Type] Search` · `[F2] Rename` · `[M] Move` · `[Del] Delete` | `shell/src/files.rs:40` | `[M]` requires `Shift+M` | `issue #69` |
| Files (Move mode) | `[Enter] Move here` · `[↑ ↓] Choose` · `[Esc] Cancel` | `shell/src/files.rs:47` | Yes | `ok` |
| Files (Rename inline) | `[Enter] Save` · `[Esc] Cancel` | `shell/src/files.rs:49` | Yes | `ok` |
| Files (New folder inline) | `[Enter] Create` · `[Esc] Cancel` | `shell/src/files.rs:50` | Yes | `ok` |
| History (with versions) | `[Enter] Restore` · `[↑ ↓] Versions` · `[PgUp PgDn] Scroll` · `[Esc] Back` | `shell/src/history.rs:26` | Yes | `ok` |
| History (empty / loading / error) | `[Esc] Back` | `shell/src/history.rs:33` | Yes (fixed in this PR) | `ok` |
| Settings (editable value row) | `[← →] Change` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/settings_page.rs:458` | Yes | `ok` |
| Settings (`Wi-Fi networks` / `Sync`) | `[Enter] Open` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/settings_page.rs:450` | Yes | `ok` |
| Settings (`Update` when ready) | `[Enter] Update` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/settings_page.rs:453` | Yes | `ok` |
| Settings (`About` / non-ready `Update`) | `[↑ ↓] Move` · `[Esc] Back` | `shell/src/settings_page.rs:456` | Yes (fixed in this PR) | `ok` |
| Sync (Overview) | `[Enter] Choose` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/sync.rs:24` | Yes | `ok` |
| Sync (Self-hosted URL) | `[Enter] Pair` · `[Esc] Back` | `shell/src/sync.rs:25` | Yes | `ok` |
| Sync (Pairing) | `[Esc] Cancel` | `shell/src/sync.rs:26` | Yes | `ok` |
| Wi-Fi (List) | `[Enter] Connect` · `[↑ ↓] Move` · `[F] Forget` · `[R] Rescan` · `[Esc] Back` | `shell/src/wifi.rs:24` | Yes | `ok` |
| Wi-Fi (Password / Other SSID) | `[Enter] Connect` · `[Esc] Cancel` | `shell/src/wifi.rs:31` | Yes | `ok` |
| Export (Configure, drive ready) | `[Enter] Export` · `[← →] Change` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/export.rs:22` | Yes | `ok` |
| Export (Configure, no drive) | `[← →] Change` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/export.rs:28` | Yes | `ok` |
| Import (Configure, drive ready) | `[Enter] Import` · `[Esc] Back` | `shell/src/export.rs:30` | Yes (`← →` also cycles multiple drives) | `ok` |
| Import (Configure, no drive) | `[Esc] Back` | `shell/src/export.rs:31` | Yes | `ok` |
| Export / Import (Running) | `[Esc] Back` | `shell/src/export.rs:32` | Yes | `ok` |
| Export (Done) | `[Enter] Done` · `[E] Export again` · `[Esc] Back` | `shell/src/export.rs:33` | Yes | `ok` |
| Import (Done) | `[Enter] Done` · `[I] Import again` · `[Esc] Back` | `shell/src/export.rs:38` | Yes | `ok` |
| First run (Welcome, setting row) | `[Enter] Open` · `[↑ ↓] Move` | `shell/src/first_run.rs:27` | Yes | `ok` |
| First run (Welcome, `Start writing`) | `[Enter] Start` · `[↑ ↓] Move` | `shell/src/first_run.rs:28` | Yes | `ok` |
| First run (Picker / `Connect sync?`) | `[Enter] Choose` · `[↑ ↓] Move` · `[Esc] Back` | `shell/src/first_run.rs:26` | Yes | `ok` |

---

## 17. Keyboard Layout Audit: US (`us`) vs Swedish (`se`)

| Area | US (`us`) | Swedish (`se`) | Root Cause / Tracking |
|---|---|---|---|
| Prose typing (`å ä ö`) | `ok` | Broken: Settings `Keyboard layout` saves `"se"` to `settings.json`, never sets `XKB_DEFAULT_LAYOUT` in Cage/session. | `issue #49` |
| Letter chords (`Ctrl+K`, `Ctrl+O`, `Ctrl+N`, `Ctrl+H`, `Ctrl+E`, `Ctrl+Shift+S`, `Ctrl+Shift+R`, `Ctrl+Shift+N`, `I`/`E`, `F`/`R`) | `ok` | `ok` (all ASCII letters sit on identical/standard QWERTY positions and match via `keyval.to_lower()`). | `ok` |
| `Ctrl+,` (Settings) | `ok` | `ok` (`,` is unshifted in the same physical position on both `us` and `se`). | `ok` |
| `F1`, `F2`, `Esc`, `Enter`, arrows, `PgUp`/`PgDn`, `Home`/`End`, `Del`, `Backspace` | `ok` | `ok` (layout-independent keysyms). | `ok` |
| `Ctrl+=` / `Ctrl+-` (Editor zoom) | `ok` (on `us` `=`/`-` keys) | Broken: `=` is `Shift+0`, `+` is on the physical `-` key, `-` is on the physical `/` key; handler matches `keyval` only without keycode fallback. | `issue #50` |
| `Ctrl+/` (Key sheet) | `ok` | Broken on physical `/` key (produces `-` on `se`) and requires `Shift+7` for `/`; `Keys::handle` discards hardware `keycode`. | `issue #68` |
| `Ctrl+'` (Quotes palette) | `ok` | Works on `se` (`'` is unshifted next to Enter), but fails on layouts where `'` is shifted/dead because `QuotePalette::handle_key` matches `gdk::Key::apostrophe` only with no keycode fallback. | `issue #68` |
