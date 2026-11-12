//! Library citations used by the pandoc filter.

use std::collections::BTreeMap;

/// One cached library quote, mirroring the `citation` object of a library
/// item in `spec/02-sync-protocol.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    /// Markdown text shown in the footnote.
    pub markdown: String,
    /// Link to the quote in Nostos, when the library has one.
    pub url: Option<String>,
}

impl Citation {
    pub fn new(markdown: impl Into<String>) -> Self {
        Self {
            markdown: markdown.into(),
            url: None,
        }
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// The part of the device's library cache that export needs: text by item id.
pub trait CitationLookup {
    /// Return the cached citation for a `nostos:cite/<id>` link, if any.
    fn lookup(&self, id: &str) -> Option<Citation>;
}

/// An empty library: every citation falls back to its link label.
impl CitationLookup for () {
    fn lookup(&self, _id: &str) -> Option<Citation> {
        None
    }
}

/// A simple in-memory [`CitationLookup`], used by tests and by callers that
/// already have the library rows at hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MapCitations {
    citations: BTreeMap<String, Citation>,
}

impl MapCitations {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: impl Into<String>, citation: Citation) {
        self.citations.insert(id.into(), citation);
    }

    pub fn len(&self) -> usize {
        self.citations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.citations.is_empty()
    }
}

impl FromIterator<(String, Citation)> for MapCitations {
    fn from_iter<T: IntoIterator<Item = (String, Citation)>>(iter: T) -> Self {
        Self {
            citations: iter.into_iter().collect(),
        }
    }
}

impl CitationLookup for MapCitations {
    fn lookup(&self, id: &str) -> Option<Citation> {
        self.citations.get(id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_lookup_returns_clones_and_none() {
        let mut map = MapCitations::new();
        map.insert("a", Citation::new("Book, p. 1"));
        map.insert(
            "b",
            Citation::new("Book, p. 2").with_url("https://nostos.example/q/2"),
        );
        assert_eq!(map.lookup("a"), Some(Citation::new("Book, p. 1")));
        assert_eq!(
            map.lookup("b").and_then(|c| c.url),
            Some("https://nostos.example/q/2".to_string())
        );
        assert_eq!(map.lookup("missing"), None);
        assert_eq!(().lookup("a"), None);
    }
}
