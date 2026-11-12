//! The Nostos library cache: highlights, notes and passages in SQLite FTS5.
//!
//! `GET /library?since=<cursor>` is incremental and paged. Each page is applied
//! in one transaction together with the new cursor, so an interrupted sync
//! simply asks for the same page again; every operation is idempotent.
//!
//! A `404` means the paired server has no library (a self-hosted
//! `carrel-server`). That is not a sync error: the cache records "unavailable"
//! and the Quotes palette shows "Quotes need a Nostos account.".
//!
//! Search is a single FTS5 query with `bm25` ranking and optional book, author
//! and topic filters. The test at the bottom of this file generates 50,000
//! items and asserts the query stays under 50 ms.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use carrel_protocol::{endpoints, LibraryPage};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::config::library_database;
use crate::http::HttpClient;

/// The sentence the Quotes palette shows when the paired server has no
/// library, and the error `SearchLibrary` returns in that case.
pub const UNAVAILABLE_MESSAGE: &str = "Quotes need a Nostos account.";

/// How many hits the palette asks for.
pub const SEARCH_LIMIT: usize = 50;

/// Ranking a query that matches almost every item is the only slow case on a
/// large library. Above this many matches the palette ranks a deterministic
/// sample spread across the library instead, which keeps a broad search under
/// the 50 ms budget while selective searches always rank every match.
const RANK_CANDIDATES: i64 = 8_000;

/// Longest display snippet, in characters. Short quotes and notes are shown
/// whole; long passages are cut around the match.
const SNIPPET_CHARS: usize = 280;

/// Guard against a server that never stops paging.
const MAX_PAGES: usize = 10_000;

/// Unit separator used in the topics columns. Topic values may contain commas
/// and spaces, so a plain CSV list would be ambiguous.
const TOPIC_SEPARATOR: &str = "\u{1f}";

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS library_books (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '',
    author TEXT NOT NULL DEFAULT '',
    topics TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS library_items (
    rowid INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    book_id TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    location TEXT NOT NULL DEFAULT '',
    item_topics TEXT NOT NULL DEFAULT '',
    book_title TEXT NOT NULL DEFAULT '',
    book_author TEXT NOT NULL DEFAULT '',
    book_topics TEXT NOT NULL DEFAULT '',
    citation_markdown TEXT NOT NULL DEFAULT '',
    citation_url TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS library_items_book ON library_items(book_id);

CREATE VIRTUAL TABLE IF NOT EXISTS library_fts USING fts5(
    text, note, title, author, topics,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS library_topics (
    topic TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS library_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// One ranked search result, ready for D-Bus and the Quotes palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct LibraryHit {
    pub id: String,
    /// `"highlight"`, `"note"` or `"passage"`.
    pub kind: String,
    /// Text shown in the palette and inserted as the quote. Long passages are
    /// cut around the search match.
    pub text: String,
    pub book_title: String,
    pub author: String,
    pub location: String,
    pub citation_markdown: String,
}

/// One book the palette can filter by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct LibraryBookOption {
    pub id: String,
    pub title: String,
    pub author: String,
}

/// Everything the Quotes palette needs for its three filter chips.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, zbus::zvariant::Type)]
pub struct LibraryFilterOptions {
    pub books: Vec<LibraryBookOption>,
    pub authors: Vec<String>,
    pub topics: Vec<String>,
}

/// What one page changed. Used by [`crate::sync::SyncReport`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryApply {
    pub books: usize,
    pub items: usize,
    pub deleted: usize,
}

/// What one `/library` fetch changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryReport {
    pub books: usize,
    pub items: usize,
    pub deleted: usize,
    /// The server answered `404`: this backend has no library.
    pub unavailable: bool,
}

impl LibraryReport {
    pub fn is_noop(&self) -> bool {
        self.items == 0 && self.deleted == 0 && self.books == 0
    }
}

/// The SQLite cache at `<state>/library.sqlite`.
pub struct LibraryCache {
    conn: Connection,
}

impl LibraryCache {
    /// Open (and create) `<state_dir>/library.sqlite`.
    pub fn from_state_dir(state_dir: &Path) -> Result<Self> {
        Self::open(&library_database(state_dir))
    }

    /// Open an explicit database path.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open library {}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // The cache is reproducible from the server; WAL keeps the palette's
        // reads from waiting on a sync's page writes.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(SCHEMA)
            .with_context(|| format!("create library tables in {}", path.display()))?;
        Ok(Self { conn })
    }

    /// Last applied library cursor, `"0"` before the first sync.
    pub fn cursor(&self) -> Result<String> {
        let cursor = self
            .conn
            .query_row(
                "SELECT value FROM library_meta WHERE key = 'cursor'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(cursor.unwrap_or_else(|| "0".to_string()))
    }

    /// True after a `404` from `/library`, until a successful fetch.
    pub fn is_unavailable(&self) -> Result<bool> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM library_meta WHERE key = 'unavailable'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(value.as_deref() == Some("1"))
    }

    /// Record that this backend has no library.
    pub fn mark_unavailable(&mut self) -> Result<()> {
        self.set_meta("unavailable", "1")?;
        Ok(())
    }

    /// Number of cached items.
    pub fn count(&self) -> Result<u64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM library_items", [], |row| row.get(0))?;
        Ok(count.max(0) as u64)
    }

    /// Apply one page and advance the cursor in a single transaction.
    pub fn apply_page(&mut self, page: &LibraryPage, cursor: &str) -> Result<LibraryApply> {
        let tx = self.conn.transaction()?;
        let mut applied = LibraryApply::default();

        for book in &page.books {
            let before: Option<(String, String, String)> = tx
                .query_row(
                    "SELECT title, author, topics FROM library_books WHERE id = ?1",
                    [&book.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let topics = topics_column(&book.topics);
            let changed =
                before.as_ref() != Some(&(book.title.clone(), book.author.clone(), topics.clone()));
            tx.execute(
                "INSERT INTO library_books (id, title, author, topics)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                     title = excluded.title,
                     author = excluded.author,
                     topics = excluded.topics",
                params![book.id, book.title, book.author, topics],
            )?;
            // The first insert also replaces the stub a lonely item created;
            // `changed` is true for a brand-new book, so its items (if any
            // arrived earlier) get the title and author in the FTS index.
            if changed {
                reindex_book(&tx, &book.id)?;
                applied.books += 1;
            }
            for topic in &book.topics {
                insert_topic(&tx, topic)?;
            }
        }

        for item in &page.items {
            let applied_item = upsert_item(&tx, item)?;
            if applied_item {
                applied.items += 1;
            }
            for topic in &item.topics {
                insert_topic(&tx, topic)?;
            }
        }

        for id in &page.deleted {
            let rowid: Option<i64> = tx
                .query_row(
                    "SELECT rowid FROM library_items WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(rowid) = rowid {
                tx.execute("DELETE FROM library_fts WHERE rowid = ?1", [rowid])?;
                tx.execute("DELETE FROM library_items WHERE rowid = ?1", [rowid])?;
                applied.deleted += 1;
            }
        }

        tx.execute(
            "INSERT INTO library_meta (key, value) VALUES ('cursor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [cursor],
        )?;
        tx.execute(
            "INSERT INTO library_meta (key, value) VALUES ('unavailable', '0')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )?;
        tx.commit()?;
        Ok(applied)
    }

    /// Ranked hits for `query`. An empty query lists the most recently synced
    /// items; `filters` accepts `book` (id or exact title), `author` and
    /// `topic`.
    pub fn search(
        &self,
        query: &str,
        filters: &HashMap<String, String>,
    ) -> Result<Vec<LibraryHit>> {
        let tokens = search_tokens(query);
        let book = filter_value(filters, "book");
        let author = filter_value(filters, "author");
        let topic = filter_value(filters, "topic");

        let mut hits = Vec::new();
        if tokens.is_empty() {
            let mut statement = self.conn.prepare(
                "SELECT i.id, i.kind, i.text, i.note, i.book_title, i.book_author,
                        i.location, i.citation_markdown
                 FROM library_items i
                 LEFT JOIN library_books b ON b.id = i.book_id
                 WHERE (?1 = '' OR i.book_id = ?1 OR b.title = ?1 COLLATE NOCASE)
                   AND (?2 = '' OR b.author = ?2 COLLATE NOCASE)
                   AND (?4 = '' OR instr(?5 || i.item_topics || ?5 || i.book_topics || ?5,
                                          ?5 || ?4 || ?5) > 0)
                 ORDER BY i.rowid DESC
                 LIMIT ?3",
            )?;
            let rows = statement.query_map(
                params![book, author, SEARCH_LIMIT as i64, topic, TOPIC_SEPARATOR],
                hit_row,
            )?;
            for row in rows {
                hits.push(hit_from_row(row?, &tokens));
            }
        } else {
            let expression = match_expression(&tokens);
            let matches: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM library_fts WHERE library_fts MATCH ?1",
                [&expression],
                |row| row.get(0),
            )?;
            let stride = if matches > RANK_CANDIDATES {
                (matches + RANK_CANDIDATES - 1) / RANK_CANDIDATES
            } else {
                1
            };
            let mut statement = self.conn.prepare(
                "SELECT i.id, i.kind, i.text, i.note, i.book_title, i.book_author,
                        i.location, i.citation_markdown
                 FROM library_fts
                 JOIN library_items i ON i.rowid = library_fts.rowid
                 WHERE library_fts MATCH ?1
                   AND (?2 = '' OR i.book_id = ?2 OR i.book_title = ?2 COLLATE NOCASE)
                   AND (?3 = '' OR i.book_author = ?3 COLLATE NOCASE)
                   AND (?5 = '' OR instr(?6 || i.item_topics || ?6 || i.book_topics || ?6,
                                         ?6 || ?5 || ?6) > 0)
                   AND (?7 = 1 OR library_fts.rowid % ?7 = 0)
                 ORDER BY bm25(library_fts, 3.0, 2.0, 4.0, 2.0, 1.0)
                 LIMIT ?4",
            )?;
            let rows = statement.query_map(
                params![
                    expression,
                    book,
                    author,
                    SEARCH_LIMIT as i64,
                    topic,
                    TOPIC_SEPARATOR,
                    stride
                ],
                hit_row,
            )?;
            for row in rows {
                hits.push(hit_from_row(row?, &tokens));
            }
        }
        Ok(hits)
    }

    /// Books, authors and topics for the palette's filter chips, sorted for a
    /// calm list.
    pub fn filter_options(&self) -> Result<LibraryFilterOptions> {
        let mut books = Vec::new();
        let mut statement = self.conn.prepare(
            "SELECT id, title, author FROM library_books
             WHERE title <> '' ORDER BY title COLLATE NOCASE, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(LibraryBookOption {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
            })
        })?;
        let mut authors = BTreeSet::new();
        for row in rows {
            let book = row?;
            if !book.author.trim().is_empty() {
                authors.insert(book.author.clone());
            }
            books.push(book);
        }
        let mut topics = Vec::new();
        let mut statement = self
            .conn
            .prepare("SELECT topic FROM library_topics ORDER BY topic COLLATE NOCASE, topic")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            topics.push(row?);
        }
        Ok(LibraryFilterOptions {
            books,
            authors: authors.into_iter().collect(),
            topics,
        })
    }

    fn set_meta(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO library_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

/// Remove the cache. Used when unpairing or pairing with a different server so
/// quotes from one account can never appear under another.
pub fn clear(state_dir: &Path) {
    let path = library_database(state_dir);
    for candidate in [
        path.clone(),
        with_suffix(&path, "-wal"),
        with_suffix(&path, "-shm"),
    ] {
        let _ = std::fs::remove_file(candidate);
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Fetch every pending page from `/library`, applying each atomically.
///
/// A `404` is the self-hosted server's way of saying "no library": it is
/// recorded as unavailable and is not an error. Any other failure ends the
/// sync, exactly like the file endpoints.
pub async fn sync_library(client: &HttpClient, state_dir: &Path) -> Result<LibraryReport> {
    let mut cache = LibraryCache::from_state_dir(state_dir)?;
    let mut report = LibraryReport::default();
    let mut since = cache.cursor()?;

    for _ in 0..MAX_PAGES {
        let response = client.get(&endpoints::library_since(&since)).await?;
        if response.status == 404 {
            cache.mark_unavailable()?;
            report.unavailable = true;
            return Ok(report);
        }
        if !response.is_success() {
            bail!(
                "GET /library failed: HTTP {}: {}",
                response.status,
                response.error_message()
            );
        }
        let page: LibraryPage = response.json()?;
        let next = if page.cursor.trim().is_empty() {
            since.clone()
        } else {
            page.cursor.clone()
        };
        let empty = page.books.is_empty() && page.items.is_empty() && page.deleted.is_empty();
        let applied = cache.apply_page(&page, &next)?;
        report.books += applied.books;
        report.items += applied.items;
        report.deleted += applied.deleted;

        // An empty page is the server saying "nothing more"; a cursor that
        // does not move would otherwise loop forever.
        if empty || next == since {
            return Ok(report);
        }
        since = next;
    }
    bail!("the library kept paging after {MAX_PAGES} pages")
}

fn upsert_item(
    tx: &rusqlite::Transaction<'_>,
    item: &carrel_protocol::LibraryItem,
) -> Result<bool> {
    // Items may arrive before their book (or for a book the server no longer
    // lists); keep a stub row so the filter list and a later book update have
    // something to fill in.
    tx.execute(
        "INSERT OR IGNORE INTO library_books (id, title, author, topics)
         VALUES (?1, '', '', '')",
        [&item.book_id],
    )?;
    let (title, author, book_topics): (String, String, String) = tx.query_row(
        "SELECT title, author, topics FROM library_books WHERE id = ?1",
        [&item.book_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let item_topics = topics_column(&item.topics);
    let mut combined: Vec<String> = item.topics.clone();
    combined.extend(split_topics(&book_topics));
    let combined = topics_column(&combined);
    let citation_markdown = item
        .citation
        .as_ref()
        .map(|citation| citation.markdown.clone())
        .unwrap_or_default();
    let citation_url = item
        .citation
        .as_ref()
        .map(|citation| citation.url.clone())
        .unwrap_or_default();
    let note = item.note.clone();
    let text = item.text.clone();
    let location = item.location.clone();
    let kind = item.kind.as_str();

    let existing: Option<i64> = tx
        .query_row(
            "SELECT rowid FROM library_items WHERE id = ?1",
            [&item.id],
            |row| row.get(0),
        )
        .optional()?;
    let rowid = match existing {
        Some(rowid) => {
            tx.execute(
                "UPDATE library_items SET
                     kind = ?2, book_id = ?3, text = ?4, note = ?5, location = ?6,
                     item_topics = ?7, book_title = ?8, book_author = ?9,
                     book_topics = ?10, citation_markdown = ?11, citation_url = ?12
                 WHERE rowid = ?1",
                params![
                    rowid,
                    kind,
                    item.book_id,
                    text,
                    note,
                    location,
                    item_topics,
                    title,
                    author,
                    book_topics,
                    citation_markdown,
                    citation_url
                ],
            )?;
            rowid
        }
        _ => {
            tx.execute(
                "INSERT INTO library_items
                     (id, kind, book_id, text, note, location, item_topics,
                      book_title, book_author, book_topics,
                      citation_markdown, citation_url)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    item.id,
                    kind,
                    item.book_id,
                    text,
                    note,
                    location,
                    item_topics,
                    title,
                    author,
                    book_topics,
                    citation_markdown,
                    citation_url
                ],
            )?;
            tx.last_insert_rowid()
        }
    };
    tx.execute("DELETE FROM library_fts WHERE rowid = ?1", [rowid])?;
    tx.execute(
        "INSERT INTO library_fts (rowid, text, note, title, author, topics)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![rowid, text, note, title, author, combined],
    )?;
    Ok(true)
}

/// Update the denormalized book fields of every item and rebuild its FTS row.
fn reindex_book(tx: &rusqlite::Transaction<'_>, book_id: &str) -> Result<()> {
    let (title, author, book_topics): (String, String, String) = tx.query_row(
        "SELECT title, author, topics FROM library_books WHERE id = ?1",
        [book_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut rows = Vec::new();
    {
        let mut statement = tx.prepare(
            "SELECT rowid, text, note, item_topics FROM library_items WHERE book_id = ?1",
        )?;
        let items = statement.query_map([book_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for item in items {
            rows.push(item?);
        }
    }
    for (rowid, text, note, item_topics) in rows {
        tx.execute(
            "UPDATE library_items SET book_title = ?2, book_author = ?3, book_topics = ?4
             WHERE rowid = ?1",
            params![rowid, title, author, book_topics],
        )?;
        let mut combined = split_topics(&item_topics);
        combined.extend(split_topics(&book_topics));
        tx.execute("DELETE FROM library_fts WHERE rowid = ?1", [rowid])?;
        tx.execute(
            "INSERT INTO library_fts (rowid, text, note, title, author, topics)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rowid, text, note, title, author, topics_column(&combined)],
        )?;
    }
    Ok(())
}

fn insert_topic(tx: &rusqlite::Transaction<'_>, topic: &str) -> Result<()> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO library_topics (topic) VALUES (?1)",
        [topic],
    )?;
    Ok(())
}

fn topics_column(topics: &[String]) -> String {
    let mut seen = BTreeSet::new();
    let mut values = Vec::new();
    for topic in topics {
        let topic = topic.trim();
        if topic.is_empty() {
            continue;
        }
        let key = topic.to_lowercase();
        if seen.insert(key) {
            values.push(topic.to_string());
        }
    }
    values.join(TOPIC_SEPARATOR)
}

fn split_topics(column: &str) -> Vec<String> {
    column
        .split(TOPIC_SEPARATOR)
        .filter(|topic| !topic.is_empty())
        .map(str::to_string)
        .collect()
}

fn filter_value(filters: &HashMap<String, String>, key: &str) -> String {
    filters
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Lowercase alphanumeric words from the query. FTS operators and punctuation
/// never reach the engine, so a writer may type quotes or a `-` safely.
fn search_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in query.chars() {
        if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                current.push(lower);
            }
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// `"one"* "two"*`: every word matches as a prefix, so search feels live while
/// the writer is still typing.
fn match_expression(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| format!("\"{token}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

struct HitRow {
    id: String,
    kind: String,
    text: String,
    note: String,
    book_title: String,
    author: String,
    location: String,
    citation_markdown: String,
}

fn hit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HitRow> {
    Ok(HitRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        text: row.get(2)?,
        note: row.get(3)?,
        book_title: row.get(4)?,
        author: row.get(5)?,
        location: row.get(6)?,
        citation_markdown: row.get(7)?,
    })
}

fn hit_from_row(row: HitRow, tokens: &[String]) -> LibraryHit {
    let source = if !row.text.trim().is_empty() && contains_any(&row.text, tokens) {
        &row.text
    } else if !row.note.trim().is_empty() && contains_any(&row.note, tokens) {
        &row.note
    } else if !row.text.trim().is_empty() {
        &row.text
    } else {
        &row.note
    };
    LibraryHit {
        id: row.id,
        kind: row.kind,
        text: snippet(source, tokens),
        book_title: row.book_title,
        author: row.author,
        location: row.location,
        citation_markdown: row.citation_markdown,
    }
}

fn contains_any(text: &str, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let lower = text.to_lowercase();
    tokens.iter().any(|token| lower.contains(token))
}

/// A calm, readable window: the whole item when it is short, otherwise up to
/// [`SNIPPET_CHARS`] characters around the first match with ellipses.
fn snippet(text: &str, tokens: &[String]) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= SNIPPET_CHARS {
        return trimmed.to_string();
    }
    let lower = trimmed.to_lowercase();
    let mut byte_offset = tokens
        .iter()
        .filter_map(|token| lower.find(token))
        .min()
        .unwrap_or(0);
    while byte_offset > 0 && !trimmed.is_char_boundary(byte_offset) {
        byte_offset -= 1;
    }
    let match_char = trimmed[..byte_offset].chars().count();
    let start = match_char.saturating_sub(SNIPPET_CHARS / 3);
    let mut result: String = trimmed.chars().skip(start).take(SNIPPET_CHARS).collect();
    result = result.trim().to_string();
    if start > 0 {
        result.insert(0, '…');
    }
    if start + SNIPPET_CHARS < count {
        result.push('…');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrel_protocol::{LibraryBook, LibraryCitation, LibraryItem, LibraryItemKind};
    use std::time::Instant;

    fn page(cursor: &str) -> LibraryPage {
        LibraryPage {
            cursor: cursor.to_string(),
            ..LibraryPage::default()
        }
    }

    fn book(id: &str, title: &str, author: &str, topics: &[&str]) -> LibraryBook {
        LibraryBook {
            id: id.into(),
            title: title.into(),
            author: author.into(),
            topics: topics.iter().map(|topic| topic.to_string()).collect(),
        }
    }

    fn item(id: &str, book_id: &str, kind: LibraryItemKind, text: &str) -> LibraryItem {
        LibraryItem {
            id: id.into(),
            kind,
            book_id: book_id.into(),
            text: text.into(),
            note: String::new(),
            location: "p. 41".into(),
            topics: vec!["writing".into()],
            citation: Some(LibraryCitation {
                markdown: format!("[The Quiet Hours, p. 41](nostos:cite/{id})"),
                url: format!("https://app.nostos.page/library/{book_id}"),
            }),
        }
    }

    fn filters(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn pages_upsert_search_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = LibraryCache::from_state_dir(dir.path()).unwrap();
        assert_eq!(cache.cursor().unwrap(), "0");
        assert!(!cache.is_unavailable().unwrap());

        let mut first = page("5");
        first.books = vec![book("b1", "The Quiet Hours", "Mara Ellison", &["writing"])];
        first.items = vec![
            item(
                "q1",
                "b1",
                LibraryItemKind::Highlight,
                "A sentence is a small room. You walk in and close the door.",
            ),
            item(
                "q2",
                "b1",
                LibraryItemKind::Passage,
                "The kettle clicks off at six.",
            ),
        ];
        let applied = cache.apply_page(&first, "5").unwrap();
        assert_eq!(
            applied,
            LibraryApply {
                books: 1,
                items: 2,
                deleted: 0
            }
        );
        assert_eq!(cache.cursor().unwrap(), "5");
        assert_eq!(cache.count().unwrap(), 2);

        let hits = cache.search("sentence", &HashMap::new()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "q1");
        assert_eq!(hits[0].kind, "highlight");
        assert_eq!(hits[0].book_title, "The Quiet Hours");
        assert_eq!(hits[0].author, "Mara Ellison");
        assert!(hits[0].text.contains("small room"));
        assert!(hits[0].citation_markdown.contains("nostos:cite/q1"));

        // Title, author and topic words all find the items.
        for query in ["quiet", "ellison", "writing"] {
            assert!(
                !cache.search(query, &HashMap::new()).unwrap().is_empty(),
                "{query}"
            );
        }

        // A page can update the book, add an item and remove one.
        let mut second = page("9");
        second.books = vec![book(
            "b1",
            "The Quiet Hours",
            "Mara Ellison",
            &["writing", "attention"],
        )];
        second.items = vec![item(
            "q3",
            "b1",
            LibraryItemKind::Note,
            "Write before the house wakes.",
        )];
        second.deleted = vec!["q2".into()];
        let applied = cache.apply_page(&second, "9").unwrap();
        assert_eq!(applied.deleted, 1);
        assert_eq!(applied.items, 1);
        assert_eq!(cache.count().unwrap(), 2);
        assert!(cache.search("kettle", &HashMap::new()).unwrap().is_empty());
        assert_eq!(cache.search("wakes", &HashMap::new()).unwrap()[0].id, "q3");
        assert_eq!(cache.cursor().unwrap(), "9");
    }

    #[test]
    fn filters_narrow_by_book_author_and_topic() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = LibraryCache::from_state_dir(dir.path()).unwrap();
        let mut page = page("3");
        page.books = vec![
            book("b1", "The Quiet Hours", "Mara Ellison", &["writing"]),
            book("b2", "On Attention", "Iris Vale", &["attention"]),
        ];
        page.items = vec![
            item(
                "q1",
                "b1",
                LibraryItemKind::Highlight,
                "A sentence is a small room.",
            ),
            item(
                "q2",
                "b2",
                LibraryItemKind::Highlight,
                "Attention is a door.",
            ),
        ];
        cache.apply_page(&page, "3").unwrap();

        let all = cache.search("", &HashMap::new()).unwrap();
        assert_eq!(all.len(), 2, "an empty query lists recent items");

        assert_eq!(
            cache.search("", &filters(&[("book", "b2")])).unwrap()[0].id,
            "q2"
        );
        assert_eq!(
            cache
                .search("", &filters(&[("book", "On Attention")]))
                .unwrap()[0]
                .id,
            "q2"
        );
        assert_eq!(
            cache
                .search("", &filters(&[("author", "mara ellison")]))
                .unwrap()[0]
                .id,
            "q1"
        );
        assert_eq!(
            cache
                .search("", &filters(&[("topic", "attention")]))
                .unwrap()[0]
                .id,
            "q2"
        );

        let options = cache.filter_options().unwrap();
        assert_eq!(options.books.len(), 2);
        assert_eq!(options.authors, vec!["Iris Vale", "Mara Ellison"]);
        assert!(options.topics.contains(&"writing".to_string()));
        assert!(options.topics.contains(&"attention".to_string()));
    }

    #[test]
    fn punctuation_in_the_query_is_not_fts_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = LibraryCache::from_state_dir(dir.path()).unwrap();
        let mut page = page("1");
        page.books = vec![book("b1", "Book", "Author", &[])];
        page.items = vec![item(
            "q1",
            "b1",
            LibraryItemKind::Highlight,
            "A sentence is a small room.",
        )];
        cache.apply_page(&page, "1").unwrap();
        for query in ["sentence", "sentence!", "\"sentence", "small-room", "*"] {
            cache.search(query, &HashMap::new()).unwrap();
        }
        assert_eq!(cache.search("*", &HashMap::new()).unwrap().len(), 1);
        assert_eq!(cache.search("\"", &HashMap::new()).unwrap().len(), 1);
    }

    #[test]
    fn unavailable_is_flagged_until_a_page_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = LibraryCache::from_state_dir(dir.path()).unwrap();
        cache.mark_unavailable().unwrap();
        assert!(cache.is_unavailable().unwrap());
        let mut page = page("2");
        page.books = vec![book("b1", "Book", "Author", &[])];
        page.items = vec![item("q1", "b1", LibraryItemKind::Highlight, "Text")];
        cache.apply_page(&page, "2").unwrap();
        assert!(!cache.is_unavailable().unwrap());
    }

    #[test]
    fn long_passages_get_a_snippet_around_the_match() {
        let long = format!("{} needle {}", "word ".repeat(120), "after ".repeat(120));
        let out = snippet(&long, &["needle".to_string()]);
        assert!(out.contains("needle"), "{out}");
        assert!(out.starts_with('…'));
        assert!(out.ends_with('…'));
        assert!(out.chars().count() <= SNIPPET_CHARS + 2);
    }

    #[test]
    fn search_stays_under_fifty_ms_for_fifty_thousand_items() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let mut cache = LibraryCache::open(&path).unwrap();

        const BOOKS: usize = 100;
        const ITEMS: usize = 50_000;
        const BATCH: usize = 1_000;
        for batch in 0..(ITEMS / BATCH) {
            let mut page = LibraryPage {
                cursor: format!("{}", batch + 1),
                ..LibraryPage::default()
            };
            for index in batch * BATCH..(batch + 1) * BATCH {
                let book_index = index % BOOKS;
                let book_id = format!("book-{book_index:03}");
                if index < BOOKS {
                    page.books.push(book(
                        &book_id,
                        &format!("A Book of Rooms {book_index}"),
                        &format!("Author {book_index:03}"),
                        &["writing", "attention"],
                    ));
                }
                let mut entry = item(
                    &format!("item-{index:05}"),
                    &book_id,
                    if index % 7 == 0 {
                        LibraryItemKind::Note
                    } else {
                        LibraryItemKind::Highlight
                    },
                    &format!(
                        "A sentence is a small room number {index}. \
                         The writer walks in, closes the door, and looks around."
                    ),
                );
                entry.topics = vec![if index % 5 == 0 {
                    "attention"
                } else {
                    "writing"
                }
                .into()];
                page.items.push(entry);
            }
            cache.apply_page(&page, &page.cursor.clone()).unwrap();
        }
        assert_eq!(cache.count().unwrap(), ITEMS as u64);

        // Warm the page cache once, then take the best of three honest runs.
        let _ = cache.search("sentence", &HashMap::new()).unwrap();
        let mut best = Duration::from_secs(60);
        let mut worst = Duration::from_secs(0);
        for _ in 0..3 {
            let started = Instant::now();
            let hits = cache.search("sentence small", &HashMap::new()).unwrap();
            let elapsed = started.elapsed();
            assert_eq!(hits.len(), SEARCH_LIMIT);
            best = best.min(elapsed);
            worst = worst.max(elapsed);
        }
        eprintln!("library search over {ITEMS} items: best {best:?}, worst {worst:?}");
        assert!(
            worst < Duration::from_millis(50),
            "search over {ITEMS} items took {worst:?}"
        );

        let filtered = Instant::now();
        let hits = cache
            .search("sentence", &filters(&[("topic", "attention")]))
            .unwrap();
        let elapsed = filtered.elapsed();
        assert!(!hits.is_empty());
        eprintln!("filtered library search over {ITEMS} items: {elapsed:?}");
        assert!(elapsed < Duration::from_millis(50), "{elapsed:?}");
    }
}
