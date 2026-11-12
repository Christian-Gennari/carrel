//! A small HTTPS + JSON client.
//!
//! The sync protocol needs exactly one thing from an HTTP stack: send a few
//! requests with a bearer token over a TLS connection whose certificate is
//! pinned by fingerprint. A hand-written HTTP/1.1 client keeps that behaviour
//! explicit and avoids dragging a second TLS stack into `stylosd`.
//!
//! Every request uses `Connection: close`. Responses may use `Content-Length`
//! or chunked encoding; both are handled.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rustls::pki_types::ServerName;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use url::Url;

use crate::tls::{client_config, ObservedCert, TlsMode};

/// Refuse to buffer more than this from one response.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Default timeout for connecting, handshaking and each read.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// A raw HTTP response. Statuses are not interpreted here so callers can
/// handle `409 Conflict` and `404 Not Found` themselves.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn text(&self) -> Result<&str> {
        std::str::from_utf8(&self.body).context("response body is not UTF-8")
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body).with_context(|| {
            format!(
                "decode JSON response (HTTP {}): {}",
                self.status,
                self.preview()
            )
        })
    }

    /// The server's `error` field when present, otherwise a short preview.
    pub fn error_message(&self) -> String {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&self.body) {
            if let Some(Value::String(message)) = map.get("error") {
                return message.clone();
            }
        }
        self.preview()
    }

    fn preview(&self) -> String {
        let text = String::from_utf8_lossy(&self.body);
        let trimmed = text.trim();
        trimmed.chars().take(300).collect()
    }
}

/// An HTTPS client bound to one server URL and optional credentials.
#[derive(Clone)]
pub struct HttpClient {
    inner: Arc<Inner>,
    authorization: Option<String>,
}

struct Inner {
    server_url: String,
    host: String,
    port: u16,
    server_name: ServerName<'static>,
    base_path: String,
    connector: TlsConnector,
    observed: ObservedCert,
    timeout: Duration,
}

impl HttpClient {
    /// Create a client for `server_url`. Only `https://` URLs are accepted.
    pub fn new(server_url: &str, mode: TlsMode) -> Result<Self> {
        let parsed = Url::parse(server_url).with_context(|| format!("parse URL {server_url}"))?;
        if parsed.scheme() != "https" {
            bail!("stylos sync requires an https:// URL, got {server_url:?}");
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            bail!("server URL must not contain a query or fragment");
        }
        let host = parsed
            .host_str()
            .context("server URL has no host")?
            .to_string();
        let port = parsed.port_or_known_default().unwrap_or(443);
        let base_path = parsed.path().trim_end_matches('/').to_string();
        let server_name = if let Ok(ip) = host.parse::<IpAddr>() {
            ServerName::IpAddress(ip.into())
        } else {
            ServerName::try_from(host.clone())
                .map_err(|error| anyhow::anyhow!("invalid server host {host:?}: {error}"))?
        };
        let (config, observed) = client_config(&mode)?;
        let connector = TlsConnector::from(Arc::new(config));
        Ok(Self {
            inner: Arc::new(Inner {
                server_url: server_url.to_string(),
                host,
                port,
                server_name,
                base_path,
                connector,
                observed,
                timeout: DEFAULT_TIMEOUT,
            }),
            authorization: None,
        })
    }

    /// Same client with a bearer token for `/files` requests.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.authorization = Some(format!("Bearer {}", token.into()));
        self
    }

    /// Set the raw `Authorization` header. Used by tests and tools to drive
    /// the admin page (HTTP Basic); sync itself uses [`HttpClient::with_token`].
    pub fn with_authorization_header(mut self, value: impl Into<String>) -> Self {
        self.authorization = Some(value.into());
        self
    }

    pub fn server_url(&self) -> &str {
        &self.inner.server_url
    }

    /// Fingerprint observed during the handshake (only in `AcceptAny` mode).
    pub fn observed_cert_sha256(&self) -> Option<String> {
        self.inner.observed.get()
    }

    /// True when the last observed certificate is valid against the public
    /// web PKI roots for this host name (only recorded in `AcceptAny` mode).
    pub fn observed_cert_publicly_trusted(&self) -> bool {
        self.inner.observed.publicly_trusted()
    }

    /// Send one request. `path` is appended to the server URL's path.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse> {
        if !path.starts_with('/') {
            bail!("request path must start with '/': {path:?}");
        }
        let mut stream = self.connect().await?;
        let target = format!("{}{}", self.inner.base_path, path);
        let host_header = host_header(&self.inner.host, self.inner.port);
        let mut head = format!(
            "{method} {target} HTTP/1.1\r\nHost: {host_header}\r\n\
             Connection: close\r\nAccept: application/json\r\n\
             User-Agent: stylosd/{}\r\n",
            env!("CARGO_PKG_VERSION")
        );
        if let Some(authorization) = &self.authorization {
            head.push_str(&format!("Authorization: {authorization}\r\n"));
        }
        if let Some(content_type) = content_type {
            head.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        if let Some(body) = body {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");

        let write = async {
            stream.write_all(head.as_bytes()).await?;
            if let Some(body) = body {
                stream.write_all(body).await?;
            }
            stream.flush().await?;
            Ok::<(), std::io::Error>(())
        };
        tokio::time::timeout(self.inner.timeout, write)
            .await
            .context("writing request timed out")??;

        self.read_response(&mut stream).await
    }

    pub async fn get(&self, path: &str) -> Result<HttpResponse> {
        self.request("GET", path, None, None).await
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.get(path).await?;
        if !response.is_success() {
            bail!(
                "GET {path} failed: HTTP {}: {}",
                response.status,
                response.error_message()
            );
        }
        response.json()
    }

    pub async fn post_json<B: Serialize>(&self, path: &str, body: &B) -> Result<HttpResponse> {
        let bytes = serde_json::to_vec(body).context("encode request body")?;
        self.request("POST", path, Some("application/json"), Some(&bytes))
            .await
    }

    pub async fn put_json<B: Serialize>(&self, path: &str, body: &B) -> Result<HttpResponse> {
        let bytes = serde_json::to_vec(body).context("encode request body")?;
        self.request("PUT", path, Some("application/json"), Some(&bytes))
            .await
    }

    pub async fn delete(&self, path: &str) -> Result<HttpResponse> {
        self.request("DELETE", path, None, None).await
    }

    async fn connect(&self) -> Result<TlsStream<TcpStream>> {
        let connect = TcpStream::connect((self.inner.host.as_str(), self.inner.port));
        let tcp = tokio::time::timeout(self.inner.timeout, connect)
            .await
            .with_context(|| {
                format!(
                    "connect to {}:{} timed out",
                    self.inner.host, self.inner.port
                )
            })?
            .with_context(|| format!("connect to {}:{}", self.inner.host, self.inner.port))?;
        tcp.set_nodelay(true).ok();
        let handshake = self
            .inner
            .connector
            .connect(self.inner.server_name.clone(), tcp);
        tokio::time::timeout(self.inner.timeout, handshake)
            .await
            .context("TLS handshake timed out")?
            .context("TLS handshake failed")
    }

    async fn read_response(&self, stream: &mut TlsStream<TcpStream>) -> Result<HttpResponse> {
        let mut buffer = Vec::with_capacity(4096);
        let header_end = loop {
            if let Some(position) = find_header_end(&buffer) {
                break position;
            }
            if buffer.len() > 128 * 1024 {
                bail!("response headers are too large");
            }
            let mut chunk = [0u8; 4096];
            let count = self.read_some(stream, &mut chunk).await?;
            if count == 0 {
                bail!("connection closed before response headers");
            }
            buffer.extend_from_slice(&chunk[..count]);
        };

        let headers_text = std::str::from_utf8(&buffer[..header_end])
            .context("response headers are not valid UTF-8")?;
        let mut lines = headers_text.split("\r\n");
        let status_line = lines.next().context("empty HTTP response")?;
        let status = parse_status(status_line)?;
        let mut headers: Vec<(String, String)> = Vec::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }

        let mut body = buffer[header_end + 4..].to_vec();
        let chunked = headers.iter().any(|(name, value)| {
            name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked")
        });
        let content_length = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .and_then(|(_, value)| value.parse::<usize>().ok());

        if chunked {
            // Read until the terminating chunk, not until EOF: servers such as
            // Kestrel close TCP without a TLS close_notify after the last chunk,
            // which rustls reports as an error.
            while !chunked_complete(&body)? {
                if body.len() > MAX_RESPONSE_BYTES {
                    bail!("response is larger than {MAX_RESPONSE_BYTES} bytes");
                }
                let mut chunk = [0u8; 8192];
                let count = self.read_some(stream, &mut chunk).await?;
                if count == 0 {
                    bail!("connection closed before the last chunk");
                }
                body.extend_from_slice(&chunk[..count]);
            }
            body = decode_chunked(&body)?;
        } else if let Some(length) = content_length {
            if length > MAX_RESPONSE_BYTES {
                bail!("response is larger than {MAX_RESPONSE_BYTES} bytes");
            }
            while body.len() < length {
                let mut chunk = [0u8; 8192];
                let count = self.read_some(stream, &mut chunk).await?;
                if count == 0 {
                    bail!(
                        "connection closed after {} of {} response bytes",
                        body.len(),
                        length
                    );
                }
                body.extend_from_slice(&chunk[..count]);
            }
            body.truncate(length);
        } else {
            self.read_to_end(stream, &mut body).await?;
        }

        Ok(HttpResponse { status, body })
    }

    async fn read_some(
        &self,
        stream: &mut TlsStream<TcpStream>,
        chunk: &mut [u8],
    ) -> Result<usize> {
        tokio::time::timeout(self.inner.timeout, stream.read(chunk))
            .await
            .context("reading response timed out")?
            .context("read response")
    }

    async fn read_to_end(
        &self,
        stream: &mut TlsStream<TcpStream>,
        body: &mut Vec<u8>,
    ) -> Result<()> {
        loop {
            if body.len() > MAX_RESPONSE_BYTES {
                bail!("response is larger than {MAX_RESPONSE_BYTES} bytes");
            }
            let mut chunk = [0u8; 8192];
            let read = tokio::time::timeout(self.inner.timeout, stream.read(&mut chunk))
                .await
                .context("reading response timed out")?;
            let count = match read {
                Ok(count) => count,
                // A close-delimited body ends when the peer closes; many
                // servers skip close_notify, so treat that as end of stream.
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => 0,
                Err(error) => return Err(error).context("read response"),
            };
            if count == 0 {
                return Ok(());
            }
            body.extend_from_slice(&chunk[..count]);
        }
    }
}

fn host_header(host: &str, port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == 443 {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_status(line: &str) -> Result<u16> {
    let mut parts = line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        bail!("malformed status line {line:?}");
    }
    let status = parts
        .next()
        .context("status line has no status code")?
        .parse::<u16>()
        .with_context(|| format!("invalid status code in {line:?}"))?;
    Ok(status)
}

/// True when `input` holds a complete chunked body: every chunk, the zero-size
/// last chunk and the (possibly empty) trailer section ending in CRLF.
fn chunked_complete(input: &[u8]) -> Result<bool> {
    let mut position = 0usize;
    loop {
        let Some(line_end) = find_crlf(input, position) else {
            return Ok(false);
        };
        let line =
            std::str::from_utf8(&input[position..line_end]).context("chunk header is not UTF-8")?;
        let size_text = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .with_context(|| format!("invalid chunk size {size_text:?}"))?;
        position = line_end + 2;
        if size == 0 {
            // Trailer lines until an empty line.
            loop {
                let Some(end) = find_crlf(input, position) else {
                    return Ok(false);
                };
                if end == position {
                    return Ok(true);
                }
                position = end + 2;
            }
        }
        position = match position.checked_add(size + 2) {
            Some(next) if next <= input.len() => next,
            _ => return Ok(false),
        };
    }
}

/// Decode a chunked transfer body. Trailing headers are ignored.
fn decode_chunked(input: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut position = 0usize;
    loop {
        let line_end = find_crlf(input, position)
            .with_context(|| format!("chunk header at byte {position} is not terminated"))?;
        let line =
            std::str::from_utf8(&input[position..line_end]).context("chunk header is not UTF-8")?;
        let size_text = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .with_context(|| format!("invalid chunk size {size_text:?}"))?;
        position = line_end + 2;
        if size == 0 {
            return Ok(output);
        }
        if position + size > input.len() {
            bail!("truncated chunk body");
        }
        output.extend_from_slice(&input[position..position + size]);
        position += size;
        if output.len() > MAX_RESPONSE_BYTES {
            bail!("response is larger than {MAX_RESPONSE_BYTES} bytes");
        }
        if input.get(position..position + 2) != Some(b"\r\n") {
            bail!("chunk is not followed by CRLF");
        }
        position += 2;
    }
}

fn find_crlf(input: &[u8], from: usize) -> Option<usize> {
    input
        .get(from..)?
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| from + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        assert_eq!(parse_status("HTTP/1.1 200 OK").unwrap(), 200);
        assert_eq!(parse_status("HTTP/1.1 409 Conflict").unwrap(), 409);
        assert!(parse_status("SSH/1.1 200 OK").is_err());
        assert!(parse_status("HTTP/1.1 nope").is_err());
    }

    #[test]
    fn detects_complete_chunked_bodies() {
        let full = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert!(chunked_complete(full).unwrap());
        for cut in 0..full.len() {
            assert!(!chunked_complete(&full[..cut]).unwrap(), "cut at {cut}");
        }
        let trailers = b"4\r\ntest\r\n0\r\nX-Trailer: 1\r\n\r\n";
        assert!(chunked_complete(trailers).unwrap());
        assert!(!chunked_complete(&trailers[..trailers.len() - 2]).unwrap());
    }

    #[test]
    fn decodes_chunked_bodies() {
        let body = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(body).unwrap(), b"hello world");
        let extended = b"4;foo=bar\r\ntest\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(extended).unwrap(), b"test");
        assert!(decode_chunked(b"5\r\nshort").is_err());
        assert!(decode_chunked(b"zz\r\n").is_err());
    }

    #[test]
    fn rejects_non_https_urls() {
        assert!(HttpClient::new("http://127.0.0.1:8443", TlsMode::AcceptAny).is_err());
        assert!(HttpClient::new("not a url", TlsMode::AcceptAny).is_err());
        assert!(HttpClient::new("https://127.0.0.1:8443/base", TlsMode::AcceptAny).is_ok());
    }

    #[test]
    fn builds_host_headers() {
        assert_eq!(host_header("example.com", 443), "example.com");
        assert_eq!(host_header("example.com", 8443), "example.com:8443");
        assert_eq!(host_header("::1", 8443), "[::1]:8443");
    }

    #[test]
    fn finds_header_boundaries() {
        assert_eq!(find_header_end(b"a\r\n\r\nb"), Some(1));
        assert_eq!(find_header_end(b"abc"), None);
    }
}
