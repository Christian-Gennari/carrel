use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use carrel_export::{
    export, Citation, Error, ExportReport, ExportedFile, Format, MapCitations, Selection,
};
use tempfile::TempDir;
use zip::ZipArchive;

const CHAPTER: &str = "# Chapter one\n\nIt was a quiet morning.\n";

const CITATIONS: &str = "# Überlegungen\n\nA first passage: [see the quote](nostos:cite/quote-1).\n\nA second passage: [see the other quote](nostos:cite/quote-2).\n";

const NESTED_FILE: &str = "Überlegungen – Notizen.md";

struct Fixture {
    directory: TempDir,
    writing: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let writing = directory.path().join("writing");
        fs::create_dir_all(writing.join("Notes/Deep")).unwrap();
        fs::create_dir_all(writing.join(".carrel")).unwrap();
        fs::write(writing.join("Chapter one.md"), CHAPTER).unwrap();
        fs::write(
            writing.join("Notes/Reading list.md"),
            "# Reading list\n\n- One\n- Two\n",
        )
        .unwrap();
        fs::write(writing.join("Notes/Deep").join(NESTED_FILE), CITATIONS).unwrap();
        fs::write(writing.join("Notes/cover.png"), b"png").unwrap();
        fs::write(writing.join(".carrel/cache.md"), "not exported").unwrap();
        Self { directory, writing }
    }

    fn destination(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join("dest").join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn library(&self) -> MapCitations {
        let mut library = MapCitations::new();
        library.insert(
            "quote-1",
            Citation::new("Marilynne Robinson, *Gilead*, p. 12")
                .with_url("https://nostos.example/library/quote-1"),
        );
        library.insert(
            "quote-2",
            Citation::new("Ursula K. Le Guin, *The Dispossessed*, p. 40"),
        );
        library
    }
}

fn citation_file(report: &ExportReport) -> &ExportedFile {
    report
        .files
        .iter()
        .find(|file| file.source.ends_with(NESTED_FILE))
        .expect("the citation file was exported")
}

fn assert_dated_folder(report: &ExportReport) {
    let name = report
        .destination
        .file_name()
        .and_then(OsStr::to_str)
        .expect("a dated folder name");
    let date = name
        .strip_prefix("Carrel export ")
        .unwrap_or_else(|| panic!("unexpected export folder {name:?}"));
    assert_eq!(date.len(), 10, "{name:?}");
    let bytes = date.as_bytes();
    assert_eq!(bytes[4], b'-');
    assert_eq!(bytes[7], b'-');
    for part in [&date[..4], &date[5..7], &date[8..]] {
        assert!(
            part.chars().all(|c| c.is_ascii_digit()),
            "not a date: {name:?}"
        );
    }
}

#[test]
fn exports_the_whole_tree_in_every_format() {
    let fixture = Fixture::new();
    let library = fixture.library();

    for format in Format::ALL {
        let destination = fixture.destination(&format!("everything-{}", format.extension()));
        let report = export(
            Selection::Everything,
            format,
            &fixture.writing,
            &destination,
            &library,
            |_| {},
        )
        .unwrap_or_else(|error| panic!("{format} export failed: {error}"));

        assert_eq!(report.format, format);
        assert_eq!(report.count(), 3, "{format}: unexpected file count");
        assert_eq!(
            report.destination.parent(),
            Some(destination.as_path()),
            "{format}: dated folder in the wrong place"
        );
        assert_dated_folder(&report);

        let nested = report
            .destination
            .join("Notes/Deep")
            .join(format!("Überlegungen – Notizen.{}", format.extension()));
        assert!(nested.is_file(), "{format}: {nested:?} is missing");

        for file in &report.files {
            assert!(file.destination.is_file(), "{format}: {file:?} is missing");
            assert_eq!(
                file.destination.extension().and_then(OsStr::to_str),
                Some(format.extension()),
                "{format}: wrong extension for {file:?}"
            );
            assert!(
                !file.destination.to_string_lossy().contains(".carrel"),
                "{format}: hidden state was exported"
            );
        }

        match format {
            Format::Markdown => assert_markdown(&report),
            Format::PlainText => assert_plain_text(&report),
            Format::Docx => {
                for file in &report.files {
                    assert_docx(&file.destination);
                }
                assert_docx_footnotes(&citation_file(&report).destination);
            }
            Format::Odt => {
                for file in &report.files {
                    assert_odt(&file.destination);
                }
            }
            Format::Pdf => {
                for file in &report.files {
                    assert_pdf(&file.destination);
                }
                let text = pdf_text(&citation_file(&report).destination);
                for word in ["Marilynne", "Gilead", "Ursula", "Dispossessed"] {
                    assert!(
                        text.contains(word),
                        "PDF extracted text lacks {word:?}: {text}"
                    );
                }
            }
            Format::Html => {
                for file in &report.files {
                    assert_html(&file.destination);
                }
                let html = fs::read_to_string(&citation_file(&report).destination).unwrap();
                assert!(html.contains("Marilynne"), "HTML lacks the library text");
                assert!(html.contains("Ursula"), "HTML lacks the second citation");
            }
            Format::Epub => {
                for file in &report.files {
                    assert_epub(&file.destination);
                }
                let epub_text = citation_text(&citation_file(&report).destination);
                assert!(
                    epub_text.contains("Marilynne"),
                    "EPUB lacks the footnote text"
                );
            }
        }
    }
}

fn assert_markdown(report: &ExportReport) {
    for file in &report.files {
        assert_eq!(
            fs::read(&file.destination).unwrap(),
            fs::read(&file.source).unwrap(),
            "Markdown must be copied unchanged"
        );
    }
    let content = fs::read_to_string(&citation_file(report).destination).unwrap();
    assert!(content.contains("nostos:cite/quote-1"));
    assert!(content.contains("nostos:cite/quote-2"));
}

fn assert_plain_text(report: &ExportReport) {
    for file in &report.files {
        let text = fs::read_to_string(&file.destination).unwrap();
        assert!(
            !text.contains("nostos:cite"),
            "plain text kept a link target"
        );
        assert!(!text.contains("# "), "plain text kept a heading marker");
    }
    let chapter = report
        .files
        .iter()
        .find(|file| file.source.ends_with("Chapter one.md"))
        .expect("chapter exported");
    let text = fs::read_to_string(&chapter.destination).unwrap();
    assert_eq!(text, "Chapter one\n\nIt was a quiet morning.\n");

    let citations = fs::read_to_string(&citation_file(report).destination).unwrap();
    assert!(citations.contains("see the quote"));
    assert!(citations.contains("A second passage: see the other quote."));
}

fn zip_names(path: &Path) -> Vec<String> {
    let file = fs::File::open(path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
        .collect()
}

fn zip_text(path: &Path, entry: &str) -> String {
    let file = fs::File::open(path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut entry = archive
        .by_name(entry)
        .unwrap_or_else(|error| panic!("{path:?} has no {entry}: {error}"));
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    text
}

fn assert_docx(path: &Path) {
    let names = zip_names(path);
    assert!(
        names.iter().any(|name| name == "[Content_Types].xml"),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name == "word/document.xml"),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name == "word/footnotes.xml"),
        "{names:?}"
    );
}

fn assert_docx_footnotes(path: &Path) {
    let notes = zip_text(path, "word/footnotes.xml");
    assert!(notes.contains("Marilynne"), "docx footnotes: {notes}");
    assert!(notes.contains("Ursula"), "docx footnotes: {notes}");
}

fn assert_odt(path: &Path) {
    let names = zip_names(path);
    assert!(names.iter().any(|name| name == "mimetype"), "{names:?}");
    assert!(names.iter().any(|name| name == "content.xml"), "{names:?}");
    assert_eq!(
        zip_text(path, "mimetype"),
        "application/vnd.oasis.opendocument.text"
    );
}

fn assert_epub(path: &Path) {
    let names = zip_names(path);
    assert!(names.iter().any(|name| name == "mimetype"), "{names:?}");
    assert!(
        names.iter().any(|name| name == "META-INF/container.xml"),
        "{names:?}"
    );
    assert!(
        names.iter().any(|name| name.ends_with("content.opf")),
        "{names:?}"
    );
    assert_eq!(zip_text(path, "mimetype"), "application/epub+zip");
}

/// Text of every XHTML entry of an EPUB, to look for footnote text.
fn citation_text(path: &Path) -> String {
    let file = fs::File::open(path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().unwrap().to_string())
        .filter(|name| name.ends_with(".xhtml"))
        .collect();
    let mut text = String::new();
    for name in names {
        text.push_str(&zip_text(path, &name));
    }
    text
}

fn assert_pdf(path: &Path) {
    let bytes = fs::read(path).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "{path:?} is not a PDF");
}

fn pdf_text(path: &Path) -> String {
    let output = Command::new("pdftotext")
        .arg(path)
        .arg("-")
        .output()
        .expect("pdftotext (poppler-utils) is needed for export tests");
    assert!(
        output.status.success(),
        "pdftotext failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_html(path: &Path) {
    let html = fs::read_to_string(path).unwrap();
    assert!(html.contains("<html"), "{path:?} is not standalone HTML");
}

#[test]
fn missing_library_entry_falls_back_to_the_link_label() {
    let directory = tempfile::tempdir().unwrap();
    let writing = directory.path().join("writing");
    fs::create_dir_all(&writing).unwrap();
    fs::write(
        writing.join("quote.md"),
        "Before [the label](nostos:cite/not-cached) after.\n",
    )
    .unwrap();
    let destination = directory.path().join("dest");
    fs::create_dir_all(&destination).unwrap();

    let report = export(
        Selection::Everything,
        Format::Html,
        &writing,
        &destination,
        &(),
        |_| {},
    )
    .unwrap();
    let html = fs::read_to_string(&report.files[0].destination).unwrap();
    assert!(
        html.contains("footnote"),
        "no footnote was generated: {html}"
    );
    assert!(
        html.contains("the label"),
        "the link label is not the footnote: {html}"
    );
}

#[test]
fn exports_a_single_file_to_the_destination_root() {
    let fixture = Fixture::new();
    let destination = fixture.destination("single");
    let report = export(
        Selection::File(PathBuf::from("Chapter one.md")),
        Format::Markdown,
        &fixture.writing,
        &destination,
        &fixture.library(),
        |_| {},
    )
    .unwrap();

    assert_eq!(report.count(), 1);
    assert_eq!(report.destination, destination);
    assert_eq!(
        report.files[0].destination,
        destination.join("Chapter one.md")
    );
    assert_eq!(
        fs::read_to_string(&report.files[0].destination).unwrap(),
        CHAPTER
    );

    // Absolute paths inside the writing root are accepted too.
    let absolute = fixture.writing.join("Chapter one.md");
    let report = export(
        Selection::File(absolute),
        Format::PlainText,
        &fixture.writing,
        &destination,
        &fixture.library(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.count(), 1);
}

#[test]
fn rejects_files_outside_the_writing_root() {
    let fixture = Fixture::new();
    let outside = fixture.directory.path().join("outside.md");
    fs::write(&outside, "secret").unwrap();
    let destination = fixture.destination("outside");

    let error = export(
        Selection::File(outside),
        Format::Markdown,
        &fixture.writing,
        &destination,
        &(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(error, Error::OutsideWritingRoot(_)), "{error:?}");
}

#[test]
fn exports_a_folder_with_its_structure() {
    let fixture = Fixture::new();
    let destination = fixture.destination("folder");
    let report = export(
        Selection::Folder(PathBuf::from("Notes")),
        Format::Markdown,
        &fixture.writing,
        &destination,
        &fixture.library(),
        |_| {},
    )
    .unwrap();

    assert_eq!(report.count(), 2);
    assert_eq!(report.destination, destination.join("Notes"));
    assert!(report.destination.join("Reading list.md").is_file());
    assert!(report.destination.join("Deep").join(NESTED_FILE).is_file());
    assert!(!report.destination.join("cover.png").exists());
}

#[test]
fn never_overwrites_files_or_export_folders() {
    let fixture = Fixture::new();
    let destination = fixture.destination("overwrite");
    let library = fixture.library();

    let first = export(
        Selection::File(PathBuf::from("Chapter one.md")),
        Format::Markdown,
        &fixture.writing,
        &destination,
        &library,
        |_| {},
    )
    .unwrap();
    let second = export(
        Selection::File(PathBuf::from("Chapter one.md")),
        Format::Markdown,
        &fixture.writing,
        &destination,
        &library,
        |_| {},
    )
    .unwrap();

    assert_eq!(
        first.files[0].destination,
        destination.join("Chapter one.md")
    );
    assert_eq!(
        second.files[0].destination,
        destination.join("Chapter one (2).md")
    );
    assert_eq!(
        fs::read_to_string(destination.join("Chapter one.md")).unwrap(),
        CHAPTER
    );
    assert_eq!(
        fs::read_to_string(destination.join("Chapter one (2).md")).unwrap(),
        CHAPTER
    );

    let first = export(
        Selection::Everything,
        Format::PlainText,
        &fixture.writing,
        &destination,
        &library,
        |_| {},
    )
    .unwrap();
    let second = export(
        Selection::Everything,
        Format::PlainText,
        &fixture.writing,
        &destination,
        &library,
        |_| {},
    )
    .unwrap();
    assert_ne!(first.destination, second.destination);
    assert_eq!(first.destination.parent(), Some(destination.as_path()));
    assert_eq!(second.destination.parent(), Some(destination.as_path()));
    assert!(second
        .destination
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.ends_with(" (2)")));
}

#[test]
fn reports_progress_for_every_file() {
    let fixture = Fixture::new();
    let destination = fixture.destination("progress");
    let mut seen = Vec::new();
    let report = export(
        Selection::Everything,
        Format::Markdown,
        &fixture.writing,
        &destination,
        &fixture.library(),
        |progress| seen.push(progress),
    )
    .unwrap();

    assert_eq!(seen.len(), report.count());
    for (position, progress) in seen.iter().enumerate() {
        assert_eq!(progress.index, position + 1);
        assert_eq!(progress.total, report.count());
        assert!(progress.destination.is_file());
        assert_eq!(
            progress.source.file_name(),
            progress.destination.file_name()
        );
    }
}

#[test]
fn plain_text_sources_are_copied_verbatim() {
    let directory = tempfile::tempdir().unwrap();
    let writing = directory.path().join("writing");
    fs::create_dir_all(&writing).unwrap();
    let body = "# not a heading\n\n**stars** and *no* conversion.\n";
    fs::write(writing.join("already.txt"), body).unwrap();
    let destination = directory.path().join("dest");
    fs::create_dir_all(&destination).unwrap();

    let report = export(
        Selection::File(PathBuf::from("already.txt")),
        Format::PlainText,
        &writing,
        &destination,
        &(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.files[0].destination, destination.join("already.txt"));
    assert_eq!(
        fs::read_to_string(&report.files[0].destination).unwrap(),
        body
    );

    // Markdown export keeps the bytes and takes the `.md` extension.
    let report = export(
        Selection::File(PathBuf::from("already.txt")),
        Format::Markdown,
        &writing,
        &destination,
        &(),
        |_| {},
    )
    .unwrap();
    assert_eq!(report.files[0].destination, destination.join("already.md"));
    assert_eq!(
        fs::read_to_string(&report.files[0].destination).unwrap(),
        body
    );
}

#[test]
fn empty_selection_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let writing = directory.path().join("writing");
    fs::create_dir_all(&writing).unwrap();
    let destination = directory.path().join("dest");
    fs::create_dir_all(&destination).unwrap();

    let error = export(
        Selection::Everything,
        Format::Markdown,
        &writing,
        &destination,
        &(),
        |_| {},
    )
    .unwrap_err();
    assert!(matches!(error, Error::NothingToExport(_)), "{error:?}");
}
