//! Ergonomic in-process HTTP test client.
//!
//! [`TestClient`] binds an [`AetherApp`] to an ephemeral port on `127.0.0.1` and speaks real
//! HTTP/1.1 over TCP, so tests exercise the full server path (parser, router, middleware,
//! timeouts) instead of a mocked handler. One TCP connection is used per request and dropped
//! after the response is read.
//!
//! ```rust,no_run
//! use aether::prelude::*;
//!
//! # async fn example() {
//! let mut app = AetherApp::new();
//! app.get("/hello", |_| "hello");
//!
//! let client = TestClient::start(app).await;
//! let res = client.get("/hello").send().await;
//! assert_eq!(res.status, 200);
//! assert_eq!(res.text(), "hello");
//! client.shutdown().await;
//! # }
//! ```

use std::borrow::Cow;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

use aether_http::method::Method;

use crate::app::AetherApp;

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HEAD_BYTES: usize = 1024 * 1024;

type ServeResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A running instance of the app under test.
///
/// The server binds `127.0.0.1:0`; use [`Self::addr`] or [`Self::url`] to reach it. It stops
/// on [`Self::shutdown`] (clean, awaited) or when the value is dropped (best-effort signal).
pub struct TestClient {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<ServeResult>>,
}

impl TestClient {
    /// Start `app` on an ephemeral loopback port.
    pub async fn start(app: AetherApp) -> Self {
        let server = app
            .bind("127.0.0.1:0")
            .await
            .expect("TestClient: bind 127.0.0.1:0 failed");
        let addr = server.local_addr();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });
        Self {
            addr,
            shutdown: Some(shutdown_tx),
            task: Some(task),
        }
    }

    /// The address the kernel assigned (always `127.0.0.1:<ephemeral>`).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Absolute `http://` URL for `path`, e.g. `url("/users/1")`.
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Begin a request with an explicit method.
    pub fn request(&self, method: Method, path: &str) -> TestRequest<'_> {
        TestRequest {
            client: self,
            method,
            path: path.to_string(),
            headers: Vec::new(),
            body: Bytes::new(),
        }
    }

    pub fn get(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Get, path)
    }

    pub fn post(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Post, path)
    }

    pub fn put(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Put, path)
    }

    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Delete, path)
    }

    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Patch, path)
    }

    pub fn head(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Head, path)
    }

    pub fn options(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::Options, path)
    }

    /// Stop the server and wait (up to 10 s) for its task to finish.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        }
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Builder for a single request. Create it via the method helpers on [`TestClient`].
pub struct TestRequest<'a> {
    client: &'a TestClient,
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: Bytes,
}

impl<'a> TestRequest<'a> {
    /// Append a request header. Repeated names are sent as repeated field lines.
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Set the request body. `Content-Length` is added automatically when absent.
    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self
    }

    /// Serialize `value` as JSON and set `Content-Type: application/json`.
    pub fn json(mut self, value: &impl serde::Serialize) -> Result<Self, serde_json::Error> {
        self.body = Bytes::from(serde_json::to_vec(value)?);
        self.headers
            .push(("Content-Type".to_string(), "application/json".to_string()));
        Ok(self)
    }

    /// Send an `application/x-www-form-urlencoded` body built from `pairs`.
    pub fn form(mut self, pairs: &[(&str, &str)]) -> Self {
        let encoded: Vec<String> = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
            .collect();
        self.body = Bytes::from(encoded.join("&").into_bytes());
        self.headers.push((
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ));
        self
    }

    /// Send the request and wait for the full response.
    pub async fn send(self) -> TestResponse {
        let mut stream = TcpStream::connect(self.client.addr)
            .await
            .expect("TestClient: connect failed");
        stream.set_nodelay(true).ok();

        let mut head = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\n",
            self.method.as_str(),
            self.path,
            self.client.addr
        );
        let has_len = self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
        let has_te = self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"));
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if !has_len && !has_te {
            head.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        head.push_str("\r\n");

        stream
            .write_all(head.as_bytes())
            .await
            .expect("TestClient: write head failed");
        stream
            .write_all(&self.body)
            .await
            .expect("TestClient: write body failed");
        stream.flush().await.expect("TestClient: flush failed");

        read_response(&mut stream, self.method).await
    }
}

/// A parsed HTTP response returned by [`TestRequest::send`].
#[derive(Debug)]
pub struct TestResponse {
    pub status: u16,
    pub status_text: String,
    /// All response headers in wire order.
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl TestResponse {
    /// First header value for `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `true` when the status is in the 200..300 range.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Body decoded as UTF-8 (lossy).
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Body deserialized from JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }
}

async fn read_response(stream: &mut TcpStream, method: Method) -> TestResponse {
    let mut buf = BytesMut::with_capacity(8192);
    let mut tmp = [0u8; 8192];

    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut tmp))
            .await
            .expect("TestClient: timed out waiting for response head")
            .expect("TestClient: read failed");
        if read == 0 {
            panic!("TestClient: connection closed before response head completed");
        }
        buf.extend_from_slice(&tmp[..read]);
        assert!(
            buf.len() <= MAX_HEAD_BYTES,
            "TestClient: response head exceeds {MAX_HEAD_BYTES} bytes"
        );
    };

    let head_text = std::str::from_utf8(&buf[..header_end])
        .expect("TestClient: response head is not valid UTF-8");
    let mut lines = head_text.split("\r\n");
    let status_line = lines.next().expect("TestClient: empty response head");
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next().unwrap_or("");
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .expect("TestClient: malformed status line");
    let status_text = parts.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();

    // HEAD responses carry the GET headers (including Content-Length) but never a payload.
    // The server always frames response bodies with Content-Length and never emits
    // `Transfer-Encoding: chunked`; a response that does carry the header (only possible
    // from a handler that sets it by hand) is treated as having no decoded body, since
    // chunk decoding is not implemented here. When neither framing header is present the
    // message is delimited by connection close, so read to EOF.
    let no_body = method == Method::Head
        || matches!(status, 101 | 204 | 304)
        || headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"));
    let body = if no_body {
        Bytes::new()
    } else {
        match headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        {
            Some(len) => {
                let total = header_end + 4 + len;
                while buf.len() < total {
                    let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut tmp))
                        .await
                        .expect("TestClient: timed out waiting for response body")
                        .expect("TestClient: read failed");
                    if read == 0 {
                        panic!(
                            "TestClient: connection closed with {} of {len} body bytes",
                            buf.len().saturating_sub(header_end + 4)
                        );
                    }
                    buf.extend_from_slice(&tmp[..read]);
                }
                buf[header_end + 4..total].to_vec().into()
            }
            None => {
                loop {
                    let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut tmp))
                        .await
                        .expect("TestClient: timed out waiting for response body")
                        .expect("TestClient: read failed");
                    if read == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..read]);
                }
                buf[header_end + 4..].to_vec().into()
            }
        }
    };

    TestResponse {
        status,
        status_text,
        headers,
        body,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn form_encode(input: &str) -> String {
    let mut out = String::new();
    for &byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
