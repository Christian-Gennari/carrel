# stylos-export

Export and import for Stylos, as a GTK-free library. `stylosd` wires it to the
Export screen over D-Bus (WP5); nothing here touches the network.

```rust
use std::path::Path;
use stylos_export::{export, Format, Selection};

let report = export(
    Selection::Everything,
    Format::Pdf,
    Path::new("/data/writing"),
    Path::new("/media/writer/STICK"),
    &library,                 // impl CitationLookup
    |progress| { /* ExportProgress */ },
)?;
```

- **Formats**: Markdown (byte copy), plain text (Rust conversion), `.docx`,
  `.odt`, HTML and EPUB through pandoc, PDF through `pandoc --pdf-engine=typst`.
- **Selection**: one file, one folder, or everything. Everything gets a dated
  `Stylos export YYYY-MM-DD/` folder that keeps the folder structure. Existing
  files are never replaced: ` (2)`, ` (3)`, … is appended.
- **Citations**: links whose target starts with `nostos:cite/` become footnotes
  in pandoc output. `filters/citations.lua` is embedded with `include_str!`,
  written to a temp file, and receives the library text as JSON metadata. If
  the library has no entry for an id, the link label is used.
- **Hidden entries** (`.stylos/`, `.git/`, …) and symlinks are skipped.
- **Import** copies `.md`/`.txt` from a drive into
  `<writing_root>/Imported/`, turning `.txt` into `.md`, never overwriting.
- **Drives**: the `Drives` trait lists, mounts (`noexec,nosuid,nodev`) and
  powers off removable drives. `UDisksDrives` speaks udisks2 over the system
  bus; `FakeDrives` is the in-memory implementation used by tests and by the
  UI's fake-drive flow. Listing never mounts anything: the caller decides.

`pandoc`, `typst` and `pdftotext` (test only) are installed by
`dev/Dockerfile`; the OS image installs pandoc and typst via `image/mkosi.conf`
and `image/mkosi.prepare`.
