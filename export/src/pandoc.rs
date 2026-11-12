//! Thin, explicit wrapper around the pandoc binary.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::citations::Citation;
use crate::{Error, Format, Result};

const PANDOC: &str = "pandoc";

/// One timeout per file. PDF generation (pandoc + typst) is the slowest path;
/// a whole book chapter still finishes well inside this.
const TIMEOUT: Duration = Duration::from_secs(180);

/// The embedded Lua filter, written to a temp file for pandoc.
const CITATIONS_LUA: &str = include_str!("../filters/citations.lua");

/// Owns the temporary directory with the Lua filter and the per-file
/// citation metadata for one export.
pub(crate) struct Pandoc {
    directory: tempfile::TempDir,
    filter: PathBuf,
}

#[derive(Serialize)]
struct Metadata<'a> {
    citations: BTreeMap<&'a str, CitationMeta<'a>>,
}

#[derive(Serialize)]
struct CitationMeta<'a> {
    markdown: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'a str>,
}

impl Pandoc {
    pub(crate) fn new() -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("stylos-export-")
            .tempdir()
            .map_err(|error| Error::io(std::env::temp_dir(), error))?;
        let filter = directory.path().join("citations.lua");
        fs::write(&filter, CITATIONS_LUA).map_err(|error| Error::io(&filter, error))?;
        Ok(Self { directory, filter })
    }

    /// Convert one Markdown file. `citations` holds the library text for every
    /// `nostos:cite/` id found in the file; missing ids fall back to the link
    /// label inside the Lua filter.
    pub(crate) fn convert(
        &self,
        format: Format,
        input: &Path,
        output: &Path,
        title: &str,
        citations: &BTreeMap<String, Citation>,
    ) -> Result<()> {
        let mut command = Command::new(PANDOC);
        command
            .arg("--from=markdown")
            .arg("--standalone")
            .arg(input)
            .arg("--output")
            .arg(output)
            .arg("--lua-filter")
            .arg(&self.filter);

        let title = if title.trim().is_empty() {
            "Untitled"
        } else {
            title
        };
        command.arg("--metadata").arg(format!("title={title}"));

        if !citations.is_empty() {
            let metadata_path = self.directory.path().join("citations.json");
            let metadata = Metadata {
                citations: citations
                    .iter()
                    .map(|(id, citation)| {
                        (
                            id.as_str(),
                            CitationMeta {
                                markdown: &citation.markdown,
                                url: citation.url.as_deref(),
                            },
                        )
                    })
                    .collect(),
            };
            let json = serde_json::to_vec(&metadata)
                .map_err(|error| Error::Internal(error.to_string()))?;
            fs::write(&metadata_path, json).map_err(|error| Error::io(&metadata_path, error))?;
            command.arg("--metadata-file").arg(&metadata_path);
        }

        match format {
            Format::Docx => command.arg("--to=docx"),
            Format::Odt => command.arg("--to=odt"),
            Format::Html => command.arg("--to=html5"),
            Format::Epub => command.arg("--to=epub3"),
            Format::Pdf => command
                .arg("--pdf-engine=typst")
                // pandoc 3.1's typst template defaults `font` to an empty
                // list, which typst 0.15 rejects. Libertinus Serif is typst's
                // bundled default, so naming it keeps the intended look and
                // works on the image (`image/mkosi.prepare` pins typst).
                .arg("--variable")
                .arg("mainfont=Libertinus Serif"),
            Format::Markdown | Format::PlainText => {
                return Err(Error::Internal(format!(
                    "{format} does not go through pandoc"
                )))
            }
        };

        run(command, input, format)?;

        let metadata = fs::metadata(output).map_err(|error| Error::io(output, error))?;
        if metadata.len() == 0 {
            return Err(Error::EmptyOutput(output.to_path_buf()));
        }
        Ok(())
    }
}

fn run(mut command: Command, input: &Path, format: Format) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::MissingTool {
                tool: PANDOC,
                format,
            }
        } else {
            Error::io(input, error)
        }
    })?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return finish(&mut child, status, input),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::io(input, error));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Timeout {
                tool: PANDOC,
                path: input.to_path_buf(),
                seconds: TIMEOUT.as_secs(),
            });
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn finish(child: &mut Child, status: ExitStatus, input: &Path) -> Result<()> {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if status.success() {
        return Ok(());
    }
    let stderr = stderr.trim();
    let stderr = if stderr.chars().count() > 2000 {
        stderr.chars().take(2000).collect()
    } else {
        stderr.to_string()
    };
    Err(Error::ToolFailed {
        tool: PANDOC,
        path: input.to_path_buf(),
        status: status.to_string(),
        stderr,
    })
}
