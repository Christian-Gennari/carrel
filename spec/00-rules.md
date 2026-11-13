# Carrel rules

These ten rules decide every product question. A feature that breaks a rule
does not ship.

1. **One app owns the screen.** The compositor runs exactly one client:
   `carrel-shell`. There is no desktop, launcher, browser, terminal or app
   switching. VT switching is disabled.
2. **The network is off unless the user starts a sync.** Wi-Fi radios are
   blocked (rfkill) at all other times. Nothing runs in the background on the
   network. The only exceptions are pairing and the Wi-Fi setup screen, both
   started by the user.
3. **Nothing is lost.** No keystroke older than 1 second may be lost on a power
   cut, crash or kernel panic. Every past version is restorable.
4. **Files are plain Markdown, and they can always leave.** User text lives as
   `.md` files in normal folders. Any computer can read them without Carrel.
   Export to a USB drive (Markdown, text, .docx, .odt, PDF, HTML, EPUB) works
   with no network and no account.
5. **Fast to the cursor.** Under 2 s from lid open (resume). Under 10 s from
   cold boot on a tier 1 laptop. No splash animations, no login.
6. **Silence.** No pop-ups, toasts, badges, sounds or update prompts. The only
   modal dialogs are for destructive actions (erase disk, delete file, restore
   an old version over the current one).
7. **One way to do each thing.** One editor, a small curated set of themes (the
   Nostos themes plus High contrast), three fonts. No plugins. No alternative
   editors.
8. **Recovery is one key.** A failed update boots the previous system. A key at
   boot opens a recovery menu. A writer never needs a shell.
9. **No AI writes for the user.** The device never generates text.
10. **If it needs a browser, it is not a feature.** Everything is done on the
    device in `carrel-shell`, or on the other side (Nostos, the user's server).
