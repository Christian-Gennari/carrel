# UX baseline — keystrokes per writer task

Measured by `shell/src/gtk_tests.rs` through the real pages and the window's key dispatcher. A later UI round must not need more key presses for the same task; the test asserts these counts.

A chord counts once. Every typed character counts once. A confirmation dialog counts the keys that reach its button.

Regenerate with `dev/run.sh bash dev/ux-baseline.sh` (Xvfb, no GPU).

| Task | Keys pressed | Key sequence |
|---|---:|---|
| Open an existing file by name | 3 | Ctrl+O End Enter |
| Create a new file | 12 | Ctrl+N C h a p t e r Space t w o |
| Insert a quote with @ | 7 | @ r o o m Down Enter |
| Start a sync | 1 | Ctrl+Shift+S |
| Export the current file to USB as .docx | 5 | Ctrl+E Down Right Right Enter |
| Export again in the last format | 2 | Ctrl+E Enter |
| Compare and restore a version | 5 | Ctrl+H Down Enter Right Enter |
| Change the theme | 2 | Ctrl+, Right |
| Open a file with the command palette | 7 | Ctrl+K z e b r a Enter |
| Toggle focus mode with the command palette | 7 | Ctrl+K f o c u s Enter |
| Set the theme to Sepia with the command palette | 7 | Ctrl+K s e p i a Enter |
| Show the keyboard shortcuts | 1 | Ctrl+/ |
| Find and open a file by typing | 7 | Ctrl+O z e b r a Enter |
| Move a file into a folder | 5 | Ctrl+O Down Down Shift+M Enter |
| First run to editor with defaults | 2 | Enter Enter |
| Change font size in Settings | 6 | Ctrl+, Down Down Down Down Right |
| Pair with Nostos from the editor | 2 | Ctrl+Shift+S Enter |
| Sync when Wi-Fi is not set up yet | 2 | Ctrl+Shift+S Enter |

<!-- UI round 6 (Files, issue #27): "Find and open a file by typing" and "Move a file into a folder". -->

<!-- UI round 7 (issue #28): "First run to editor with defaults" and "Change font size in Settings" are the Settings and first-run rows. -->

<!-- Sync and Wi-Fi from the editor (issue #29): the last two rows. -->
