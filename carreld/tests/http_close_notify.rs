//! Regression test for issue #3: servers such as Kestrel (Nostos) answer with
//! `Transfer-Encoding: chunked` and then close TCP without a TLS close_notify.
//! The client must accept the complete response instead of failing with
//! "peer closed connection without sending TLS close_notify".

use std::sync::Arc;

use carreld::http::HttpClient;
use carreld::tls::TlsMode;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Serve one HTTPS response, then drop the TCP socket without close_notify.
async fn serve_once(response: &'static [u8]) -> String {
    serve_sequence(vec![response]).await
}

/// Serve the given responses on consecutive connections (self-signed cert),
/// dropping each TCP socket without close_notify.
async fn serve_sequence(responses: Vec<&'static [u8]>) -> String {
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let der = CertificateDer::from(cert.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![der], key)
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        for response in responses {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let mut request = vec![0u8; 4096];
            let _ = tls.read(&mut request).await.unwrap();
            tls.write_all(response).await.unwrap();
            tls.flush().await.unwrap();
            // Drop the raw TCP stream without sending close_notify.
            let (tcp, _) = tls.into_inner();
            drop(tcp);
        }
    });
    format!("https://127.0.0.1:{port}")
}

#[tokio::test]
async fn chunked_response_without_close_notify_is_accepted() {
    let url = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n{\"ok\":\r\n5\r\ntrue}\r\n0\r\n\r\n",
    )
    .await;
    let client = HttpClient::new(&url, TlsMode::AcceptAny).unwrap();
    let response = client.get("/anything").await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"{\"ok\":true}");
}

#[tokio::test]
async fn close_delimited_response_without_close_notify_is_accepted() {
    let url = serve_once(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nplain body").await;
    let client = HttpClient::new(&url, TlsMode::AcceptAny).unwrap();
    let response = client.get("/anything").await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"plain body");
}

/// A server that reports no certificate fingerprint expects public-CA
/// verification afterwards. With a self-signed certificate that can never
/// work, so pairing must stop with a clear error instead of "succeeding".
#[tokio::test]
async fn pairing_without_fingerprint_requires_a_publicly_trusted_certificate() {
    let url = serve_sequence(vec![
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 91\r\n\r\n{\"pair_id\":\"01a1cf76-07b7-71ac-b5d7-afe2a62af38e\",\"code\":\"K7Q-4MZ\",\"expires_at\":4102444800}",
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 42\r\n\r\n{\"status\":\"approved\",\"token\":\"stl_secret\"}",
    ])
    .await;
    let pubkey = carreld::generate_device_pubkey().unwrap();
    let session = carreld::PairingSession::start(&url, "test deck", &pubkey)
        .await
        .unwrap();
    let error = session
        .wait(
            std::time::Duration::from_millis(10),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("not trusted by a public certificate authority"),
        "{error}"
    );
}
