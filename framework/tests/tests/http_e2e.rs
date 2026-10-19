//! End-to-end tests over real TCP sockets.
//!
//! Every test binds `127.0.0.1:0`, reads the assigned port from `AetherServer::local_addr`,
//! and drives the server with raw bytes. This suite is checked in so the behaviours the
//! documentation claims (chunked bodies, duplicate headers, multipart, static ranges,
//! CORS scoping, timeouts, panic isolation, graceful shutdown, WebSocket upgrade) stay
//! verified over time.

use aether::prelude::*;
use bytes::Bytes;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

type ServeResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TestServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<ServeResult>,
}

impl TestServer {
    async fn start(app: AetherApp) -> Self {
        let server = app.bind("127.0.0.1:0").await.expect("bind ephemeral port");
        let addr = server.local_addr();
        let (shutdown, wait) = oneshot::channel();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async move {
                    let _ = wait.await;
                })
                .await
        });
        Self {
            addr,
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        match tokio::time::timeout(Duration::from_secs(10), &mut self.task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => panic!("server returned an error: {error}"),
            Ok(Err(join)) => panic!("server task failed: {join}"),
            Err(_) => panic!("server did not finish graceful shutdown within 10s"),
        }
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("body is not JSON ({error}): {:?}", String::from_utf8_lossy(&self.body)))
    }
}

struct Client {
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).await.expect("connect to test server");
        stream.set_nodelay(true).expect("set TCP_NODELAY");
        Self {
            stream,
            buffered: Vec::new(),
        }
    }

    async fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await
    }

    async fn send_request(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> std::io::Result<()> {
        let mut request = format!("{method} {path} HTTP/1.1\r\nHost: test\r\n");
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            && !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"))
        {
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        let mut bytes = request.into_bytes();
        bytes.extend_from_slice(body);
        self.send(&bytes).await
    }

    fn take_response(&mut self, expect_body: bool) -> Option<HttpResponse> {
        let header_end = find_subslice(&self.buffered, b"\r\n\r\n")?;
        let header_text = std::str::from_utf8(&self.buffered[..header_end]).ok()?;
        let mut lines = header_text.split("\r\n");
        let status = lines.next()?.split_whitespace().nth(1)?.parse::<u16>().ok()?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();

        let body_len = if expect_body {
            headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .unwrap_or(0)
        } else {
            0
        };
        let total = header_end + 4 + body_len;
        if self.buffered.len() < total {
            return None;
        }
        let body = self.buffered[header_end + 4..total].to_vec();
        self.buffered.drain(..total);
        Some(HttpResponse {
            status,
            headers,
            body,
        })
    }

    async fn response(&mut self, expect_body: bool) -> HttpResponse {
        loop {
            if let Some(response) = self.take_response(expect_body) {
                return response;
            }
            let mut chunk = [0u8; 8192];
            let read = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
                .await
                .expect("timed out waiting for a response")
                .expect("failed to read a response");
            assert!(read > 0, "connection closed before a complete response");
            self.buffered.extend_from_slice(&chunk[..read]);
        }
    }

    async fn poll_response(&mut self, expect_body: bool) -> Option<HttpResponse> {
        if let Some(response) = self.take_response(expect_body) {
            return Some(response);
        }
        let mut chunk = [0u8; 8192];
        match tokio::time::timeout(Duration::from_millis(5), self.stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) => None,
            Ok(Ok(read)) => {
                self.buffered.extend_from_slice(&chunk[..read]);
                self.take_response(expect_body)
            }
            Ok(Err(_)) => None,
        }
    }

    async fn expect_closed(&mut self) {
        let mut chunk = [0u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk)).await {
                Ok(Ok(0)) | Ok(Err(_)) => return,
                Ok(Ok(_)) => continue,
                Err(_) => panic!("server kept the connection open instead of closing it"),
            }
        }
    }

    async fn read_exact_ws(&mut self, len: usize) -> Vec<u8> {
        while self.buffered.len() < len {
            let mut chunk = [0u8; 8192];
            let read = tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk))
                .await
                .expect("timed out waiting for WebSocket bytes")
                .expect("failed to read WebSocket bytes");
            assert!(read > 0, "connection closed before the frame was complete");
            self.buffered.extend_from_slice(&chunk[..read]);
        }
        self.buffered.drain(..len).collect()
    }

    async fn read_ws_frame(&mut self) -> (u8, bool, Vec<u8>) {
        let head = self.read_exact_ws(2).await;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0f;
        assert_eq!(head[1] & 0x80, 0, "server frames must not be masked");
        let len = match head[1] & 0x7f {
            126 => {
                let ext = self.read_exact_ws(2).await;
                u16::from_be_bytes([ext[0], ext[1]]) as usize
            }
            127 => {
                let ext = self.read_exact_ws(8).await;
                u64::from_be_bytes(ext.try_into().expect("8 bytes")) as usize
            }
            n => n as usize,
        };
        let payload = self.read_exact_ws(len).await;
        (opcode, fin, payload)
    }
}

fn ws_client_frame(fin: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push((if fin { 0x80 } else { 0 }) | opcode);
    match payload.len() {
        len @ 0..=125 => out.push(0x80 | len as u8),
        len @ 126..=65535 => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index & 3]),
    );
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "aether_e2e_{}_{}_{}",
        std::process::id(),
        tag,
        counter
    ));
    std::fs::create_dir_all(&path).expect("create temporary directory");
    path
}

#[tokio::test]
async fn routing_params_405_allow_head_and_404() {
    let mut app = AetherApp::new();
    app.get("/text", |_| "hello world");
    app.get("/users/{id}", |req| {
        let id = req.param("id").unwrap_or_default().to_string();
        Response::json(Bytes::from(json!({ "id": id }).to_string()))
    });
    app.put("/users/{id}", |_| Response::status(204, "No Content"));
    app.get("/static-only", |_| "static");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/text", &[], b"").await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"hello world");
    assert_eq!(response.header("Content-Type"), Some("text/plain; charset=utf-8"));

    // Route captures are percent-decoded exactly once, after routing on the raw path.
    client.send_request("GET", "/users/%D9%85", &[], b"").await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["id"], "م");

    client.send_request("DELETE", "/users/7", &[], b"").await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 405);
    let allow = response.header("Allow").expect("405 must carry Allow");
    assert!(allow.contains("GET"), "Allow was {allow}");
    assert!(allow.contains("PUT"), "Allow was {allow}");
    assert!(!allow.contains("DELETE"), "Allow was {allow}");

    client.send_request("GET", "/missing", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 404);

    // HEAD falls back to GET, preserving the representation headers but not the payload.
    client.send_request("HEAD", "/text", &[], b"").await.unwrap();
    let response = client.response(false).await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("Content-Length"), Some("11"));
    assert!(response.body.is_empty());

    // The connection is still usable for a subsequent request.
    client.send_request("GET", "/static-only", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.body, b"static");

    server.stop().await;
}

#[tokio::test]
async fn keep_alive_pipelining_and_chunked_bodies() {
    let mut app = AetherApp::new();
    app.get("/a", |_| "A");
    app.get("/b", |_| "B");
    app.post("/echo", |req| {
        let body = String::from_utf8_lossy(&req.body).into_owned();
        Response::json(Bytes::from(json!({ "body": body, "len": req.body.len() }).to_string()))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // Two complete requests in one TCP write must be answered in order.
    let pipelined = b"GET /a HTTP/1.1\r\nHost: test\r\n\r\nGET /b HTTP/1.1\r\nHost: test\r\n\r\n";
    client.send(pipelined).await.unwrap();
    let first = client.response(true).await;
    let second = client.response(true).await;
    assert_eq!((first.status, first.body.as_slice()), (200, b"A".as_slice()));
    assert_eq!((second.status, second.body.as_slice()), (200, b"B".as_slice()));

    // A third request on the same persistent connection.
    client.send_request("GET", "/b", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.body, b"B");

    let chunked = b"POST /echo HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n\
5\r\nhello\r\n1\r\n \r\n5\r\nworld\r\n0\r\nX-Trailer: accepted\r\n\r\n";
    client.send(chunked).await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["body"], "hello world");
    assert_eq!(json["len"], 11);

    // Conflicting framing is rejected and the connection is closed.
    let smuggling = b"POST /echo HTTP/1.1\r\nHost: test\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    client.send(smuggling).await.unwrap();
    assert_eq!(client.response(true).await.status, 400);
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn duplicate_request_header_fields_are_preserved() {
    let mut app = AetherApp::new();
    app.get("/headers", |req| {
        Response::json(Bytes::from(
            json!({
                "cookies": req.header_values("cookie"),
                "dup": req.header_values("X-Dup"),
                "first": req.header("x-dup"),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /headers HTTP/1.1\r\nHost: test\r\nCookie: a=1\r\nX-Dup: one\r\ncookie: b=2\r\nx-dup: two\r\n\r\n")
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["cookies"], json!(["a=1", "b=2"]));
    assert_eq!(json["dup"], json!(["one", "two"]));
    assert_eq!(json["first"], "one");

    server.stop().await;
}

#[tokio::test]
async fn multipart_form_data_is_parsed_end_to_end() {
    let mut app = AetherApp::new();
    app.post("/upload", |req| {
        let form = match req.multipart() {
            Ok(form) => form,
            Err(error) => {
                return Response::json_status(
                    400,
                    "Bad Request",
                    Bytes::from(json!({ "error": error.to_string() }).to_string()),
                )
            }
        };
        let mut fields = serde_json::Map::new();
        let mut filename = None;
        let mut file_len = 0usize;
        for part in form.parts() {
            match part.name.as_deref() {
                Some("file") => {
                    filename = part.filename.clone();
                    file_len = part.data.len();
                }
                Some(name) => {
                    if let Ok(text) = part.text() {
                        fields.insert(name.to_string(), json!(text));
                    }
                }
                None => {}
            }
        }
        Response::json(Bytes::from(
            json!({
                "fields": fields,
                "filename": filename,
                "file_len": file_len,
                "parts": form.parts().len(),
            })
            .to_string(),
        ))
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    let boundary = "AaB03x";
    let body = format!(
        "--{boundary}\r\n\
Content-Disposition: form-data; name=\"title\"\r\n\
\r\n\
hello\r\n\
--{boundary}\r\n\
Content-Disposition: form-data; name=\"file\"; filename=\"report.txt\"\r\n\
Content-Type: text/plain\r\n\
\r\n\
file-data\r\n\
--{boundary}--\r\n"
    );
    client
        .send_request(
            "POST",
            "/upload",
            &[("Content-Type", &format!("multipart/form-data; boundary={boundary}"))],
            body.as_bytes(),
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 200);
    let json = response.json();
    assert_eq!(json["fields"]["title"], "hello");
    assert_eq!(json["filename"], "report.txt");
    assert_eq!(json["file_len"], 9);
    assert_eq!(json["parts"], 2);

    client
        .send_request(
            "POST",
            "/upload",
            &[("Content-Type", "multipart/form-data")],
            b"anything",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 400);
    assert!(response.json()["error"].as_str().unwrap().contains("boundary"));

    server.stop().await;
}

#[tokio::test]
async fn static_files_support_conditional_and_range_requests() {
    let base = temp_dir("static");
    let public = base.join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(public.join("data.txt"), b"0123456789").unwrap();
    std::fs::write(base.join("secret.txt"), b"secret").unwrap();

    let mut app = AetherApp::new();
    app.static_dir("/assets", public.clone());
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/assets/data.txt", &[], b"").await.unwrap();
    let full = client.response(true).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.body, b"0123456789");
    assert_eq!(full.header("Accept-Ranges"), Some("bytes"));
    let etag = full.header("ETag").expect("static response has an ETag").to_string();

    client
        .send_request("GET", "/assets/data.txt", &[("If-None-Match", &etag)], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 304);

    client
        .send_request("GET", "/assets/data.txt", &[("Range", "bytes=2-5")], b"")
        .await
        .unwrap();
    let partial = client.response(true).await;
    assert_eq!(partial.status, 206);
    assert_eq!(partial.body, b"2345");
    assert_eq!(partial.header("Content-Range"), Some("bytes 2-5/10"));

    client
        .send_request(
            "GET",
            "/assets/data.txt",
            &[("Range", "bytes=0-99"), ("If-Range", &etag)],
            b"",
        )
        .await
        .unwrap();
    assert_eq!(client.response(true).await.body, b"0123456789");

    client
        .send_request("GET", "/assets/data.txt", &[("Range", "bytes=99-")], b"")
        .await
        .unwrap();
    let unsatisfiable = client.response(true).await;
    assert_eq!(unsatisfiable.status, 416);
    assert_eq!(unsatisfiable.header("Content-Range"), Some("bytes */10"));

    for hostile in ["/assets/%2e%2e/secret.txt", "/assets/..%2fsecret.txt", "/assets/CON"] {
        client.send_request("GET", hostile, &[], b"").await.unwrap();
        let response = client.response(true).await;
        assert_eq!(response.status, 404, "hostile static path {hostile} served content");
    }

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn static_file_serves_disk_content_over_async_route() {
    let base = temp_dir("static_file");
    std::fs::create_dir_all(&base).unwrap();
    let file = base.join("report.css");
    std::fs::write(&file, b"body{margin:0}").unwrap();
    let served: &'static str = Box::leak(file.into_os_string().into_string().unwrap().into_boxed_str());
    let missing: &'static str =
        Box::leak(base.join("gone.css").into_os_string().into_string().unwrap().into_boxed_str());

    let mut app = AetherApp::new();
    app.static_file("/report.css", served);
    app.static_file("/gone.css", missing);
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/report.css", &[], b"").await.unwrap();
    let full = client.response(true).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.body, b"body{margin:0}");
    assert!(full.header("Content-Type").unwrap().starts_with("text/css"));

    client.send_request("GET", "/gone.css", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 404);

    server.stop().await;
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn http10_requests_close_after_one_response_by_default() {
    let mut app = AetherApp::new();
    app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send(b"GET /ping HTTP/1.0\r\nHost: t\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"pong");
    assert_eq!(
        resp.header("Connection"),
        Some("close"),
        "HTTP/1.0 has no implicit persistence (RFC 9112 §9)"
    );
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn http10_explicit_keepalive_persists_and_is_advertised() {
    let mut app = AetherApp::new();
    app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    let request = b"GET /ping HTTP/1.0\r\nHost: t\r\nConnection: keep-alive\r\n\r\n";
    client.send(request).await.unwrap();
    let first = client.response(true).await;
    assert_eq!(first.status, 200);
    assert_eq!(
        first.header("Connection"),
        Some("keep-alive"),
        "a persisted HTTP/1.0 connection must advertise it"
    );

    client.send(request).await.unwrap();
    assert_eq!(client.response(true).await.body, b"pong");

    server.stop().await;
}

#[tokio::test]
async fn http11_default_persistence_is_unchanged() {
    let mut app = AetherApp::new();
    app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    for _ in 0..2 {
        client.send_request("GET", "/ping", &[], b"").await.unwrap();
        assert_eq!(client.response(true).await.body, b"pong");
    }
    // No implicit close on 1.1: the connection is still open for a third request.
    client.send_request("GET", "/ping", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);

    server.stop().await;
}

#[tokio::test]
async fn idle_keepalive_connection_closes_after_keep_alive_timeout() {
    let config = AetherConfig {
        keep_alive_timeout: Duration::from_millis(300),
        ..AetherConfig::default()
    };
    let mut app = AetherApp::with_config(config);
    app.get("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/ping", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);
    // Idle: nothing sent, nothing received — the server must drop the socket on its own.
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn http10_chunked_request_is_rejected() {
    let mut app = AetherApp::new();
    app.post("/ping", |_| "pong");
    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // Transfer-Encoding chunked is an HTTP/1.1 framing; over 1.0 it must not be accepted.
    client
        .send(b"POST /ping HTTP/1.0\r\nHost: t\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n")
        .await
        .unwrap();
    let resp = client.response(true).await;
    assert_eq!(resp.status, 400);

    server.stop().await;
}

#[tokio::test]
async fn cors_preflight_is_scoped_to_real_routes() {
    let mut app = AetherApp::new();
    app.enable_cors(CorsMiddleware::new(CorsConfig::default()));
    app.post("/submit", |_| "submitted");
    app.route_sync(Method::Options, "/custom", |_| "custom-options");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client
        .send_request(
            "OPTIONS",
            "/submit",
            &[
                ("Origin", "https://example.com"),
                ("Access-Control-Request-Method", "POST"),
                ("Access-Control-Request-Headers", "Content-Type"),
            ],
            b"",
        )
        .await
        .unwrap();
    let preflight = client.response(true).await;
    assert_eq!(preflight.status, 204);
    assert_eq!(preflight.header("Access-Control-Allow-Origin"), Some("*"));
    assert!(preflight
        .header("Access-Control-Allow-Methods")
        .unwrap()
        .contains("POST"));

    // A registered OPTIONS handler wins over the automatic preflight response.
    client
        .send_request(
            "OPTIONS",
            "/custom",
            &[("Origin", "https://example.com"), ("Access-Control-Request-Method", "POST")],
            b"",
        )
        .await
        .unwrap();
    let custom = client.response(true).await;
    assert_eq!(
        (custom.status, custom.body.as_slice()),
        (200, b"custom-options".as_slice())
    );

    // A preflight for a non-routable path or disallowed method is refused.
    for (path, method) in [("/missing", "POST"), ("/submit", "DELETE")] {
        client
            .send_request(
                "OPTIONS",
                path,
                &[
                    ("Origin", "https://example.com"),
                    ("Access-Control-Request-Method", method),
                ],
                b"",
            )
            .await
            .unwrap();
        assert_eq!(client.response(true).await.status, 403, "preflight {method} {path}");
    }

    server.stop().await;
}

#[tokio::test]
async fn body_and_header_limits_are_enforced_with_413_and_431() {
    let mut body_config = AetherConfig::default();
    body_config.max_body_bytes = 16;
    let mut app = AetherApp::with_config(body_config);
    app.post("/echo", |req| format!("{}", req.body.len()));

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send_request("POST", "/echo", &[], &[b'x'; 20])
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 413);
    client.expect_closed().await;
    server.stop().await;

    // A small header budget must produce 431, not the connection-buffer 413 guard. Keep the
    // body budget large so the total buffered request stays under the connection limit.
    let mut header_config = AetherConfig::default();
    header_config.max_header_bytes = 64;
    let mut app = AetherApp::with_config(header_config);
    app.get("/echo", |_| "ok");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let oversized_header = "x".repeat(200);
    client
        .send_request("GET", "/echo", &[("X-Large", &oversized_header)], b"")
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 431);
    client.expect_closed().await;
    server.stop().await;
}

#[tokio::test]
async fn handler_panics_are_isolated_to_one_response() {
    let mut app = AetherApp::new();
    app.get("/ok", |_| "ok");
    app.get("/panic", |_| -> &'static str { panic!("intentional test panic") });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.send_request("GET", "/panic", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 500);

    client.send_request("GET", "/ok", &[], b"").await.unwrap();
    let ok = client.response(true).await;
    assert_eq!((ok.status, ok.body.as_slice()), (200, b"ok".as_slice()));

    server.stop().await;
}

#[tokio::test]
async fn slow_trickle_receives_408_within_the_total_deadline() {
    let mut config = AetherConfig::default();
    config.max_request_time = Duration::from_millis(300);
    let mut app = AetherApp::with_config(config);
    app.get("/slow", |_| "slow");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client
        .send(b"GET /slow HTTP/1.1\r\nHost: test\r\n")
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let response = loop {
        if let Some(response) = client.poll_response(false).await {
            break response;
        }
        if Instant::now() > deadline {
            panic!("server never enforced the total receive deadline");
        }
        let _ = client.send(b"X").await;
        tokio::time::sleep(Duration::from_millis(40)).await;
    };
    assert_eq!(response.status, 408);
    assert_eq!(response.header("Connection"), Some("close"));

    server.stop().await;
}

#[tokio::test]
async fn graceful_shutdown_stops_accepting_and_drains_keep_alive() {
    let mut app = AetherApp::new();
    app.get("/", |_| "alive");

    let server = TestServer::start(app).await;
    let addr = server.addr;
    let mut client = Client::connect(addr).await;
    client.send_request("GET", "/", &[], b"").await.unwrap();
    assert_eq!(client.response(true).await.status, 200);

    // The idle keep-alive connection is closed when shutdown starts, and the accept loop
    // finishes rather than waiting for the read timeout.
    let shutdown = tokio::spawn(async move { server.stop().await });
    tokio::time::timeout(Duration::from_secs(5), client.expect_closed())
        .await
        .expect("idle keep-alive connection was not closed on shutdown");
    shutdown.await.expect("shutdown task panicked");

    let refused = TcpStream::connect(addr).await;
    assert!(refused.is_err(), "a connection was accepted after graceful shutdown");
}

#[test]
fn static_range_parser_contract() {
    use aether_http::static_files::{parse_range_header, RangeDecision};
    assert_eq!(
        parse_range_header("bytes=1-3", 10),
        RangeDecision::Satisfiable { start: 1, end: 3 }
    );
    assert_eq!(parse_range_header("bytes=99-", 10), RangeDecision::Unsatisfiable);
    assert_eq!(parse_range_header("bytes=0-1,4-5", 10), RangeDecision::Ignore);
}

#[tokio::test]
async fn websocket_upgrade_echo_fragmentation_ping_and_close() {
    let mut app = AetherApp::new();
    app.ws("/ws/echo", |_req, mut socket| async move {
        while let Ok(Some(message)) = socket.recv().await {
            let sent = match message {
                WsMessage::Text(text) => socket.send_text(&text).await,
                WsMessage::Binary(bytes) => socket.send_binary(&bytes).await,
            };
            if sent.is_err() {
                break;
            }
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    // RFC 6455 §1.3 sample key: the accept value is the published test vector.
    client
        .send(
            b"GET /ws/echo HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: keep-alive, Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let upgrade = client.response(false).await;
    assert_eq!(upgrade.status, 101);
    assert!(upgrade.header("Upgrade").unwrap().eq_ignore_ascii_case("websocket"));
    assert!(upgrade.header("Connection").unwrap().eq_ignore_ascii_case("upgrade"));
    assert_eq!(
        upgrade.header("Sec-WebSocket-Accept"),
        Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    );
    assert!(upgrade.header("Content-Length").is_none(), "101 must not be framed as a body");

    client
        .send(&ws_client_frame(true, 0x1, b"hello", [1, 2, 3, 4]))
        .await
        .unwrap();
    let (opcode, fin, payload) = client.read_ws_frame().await;
    assert_eq!((opcode, fin), (0x1, true));
    assert_eq!(payload, b"hello");

    // Control frame interleaved with data is answered by the driver, not the handler.
    client
        .send(&ws_client_frame(true, 0x9, b"ping!", [5, 6, 7, 8]))
        .await
        .unwrap();
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0xA);
    assert_eq!(payload, b"ping!");

    // Fragmented text is reassembled before the handler sees it.
    client
        .send(&ws_client_frame(false, 0x1, b"frag", [1, 1, 1, 1]))
        .await
        .unwrap();
    client
        .send(&ws_client_frame(true, 0x0, b"mented", [2, 2, 2, 2]))
        .await
        .unwrap();
    let (opcode, fin, payload) = client.read_ws_frame().await;
    assert_eq!((opcode, fin), (0x1, true));
    assert_eq!(payload, b"fragmented");

    // Close is echoed and the TCP connection is released.
    let mut close_payload = 1000u16.to_be_bytes().to_vec();
    close_payload.extend_from_slice(b"done");
    client
        .send(&ws_client_frame(true, 0x8, &close_payload, [3, 3, 3, 3]))
        .await
        .unwrap();
    let (opcode, _, echoed) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x8);
    assert_eq!(echoed, close_payload);
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn websocket_handshake_failures_and_unmasked_frames_are_rejected() {
    let mut app = AetherApp::new();
    app.ws("/ws", |_req, mut socket| async move {
        while let Ok(Some(WsMessage::Text(text))) = socket.recv().await {
            if socket.send_text(&text).await.is_err() {
                break;
            }
        }
    });

    let server = TestServer::start(app).await;

    // Missing Sec-WebSocket-Key.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(true).await.status, 400);

    // Unsupported version advertises the only supported version.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 8\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 426);
    assert_eq!(response.header("Sec-WebSocket-Version"), Some("13"));

    // The route is GET-only; other methods are never upgraded.
    let mut client = Client::connect(server.addr).await;
    client.send_request("POST", "/ws", &[], b"").await.unwrap();
    let response = client.response(true).await;
    assert_eq!(response.status, 405);
    assert!(response.header("Allow").unwrap().contains("GET"));

    // RFC 6455 §5.1: client frames must be masked. The server closes with 1002.
    let mut client = Client::connect(server.addr).await;
    client
        .send(
            b"GET /ws HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\n\
              Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .await
        .unwrap();
    assert_eq!(client.response(false).await.status, 101);
    client.send(&[0x81, 0x02, b'h', b'i']).await.unwrap();
    let (opcode, _, payload) = client.read_ws_frame().await;
    assert_eq!(opcode, 0x8);
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 1002);
    client.expect_closed().await;

    server.stop().await;
}

#[tokio::test]
async fn error_responses_survive_unread_pipelined_bytes() {
    // Regression: closing an error-path socket without a graceful FIN makes
    // Windows send an RST over the client's still-unread request bytes, which
    // discards the 431/400 response that was just written. The client must
    // actually receive the error status line, not a connection reset.
    let mut header_config = AetherConfig::default();
    header_config.max_header_bytes = 4096;
    let mut app = AetherApp::with_config(header_config);
    app.get("/echo", |_| "ok");

    let server = TestServer::start(app).await;
    let mut stream = TcpStream::connect(server.addr).await.expect("connect");
    let mut payload = b"GET /echo HTTP/1.1\r\nHost: x\r\nX-Big: ".to_vec();
    payload.extend_from_slice(&[b'A'; 20_000]);
    payload.extend_from_slice(b"\r\n\r\n");
    stream.write_all(&payload).await.unwrap();
    // Let the server parse, respond and close WITHOUT us reading, so request
    // bytes are still sitting in its receive buffer at close time.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut buf = Vec::new();
    let read =
        tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await;
    server.stop().await;
    match read {
        Ok(Ok(_)) => {
            let text = String::from_utf8_lossy(&buf);
            let head = &text[..text.len().min(64)];
            assert!(text.starts_with("HTTP/1.1 431"), "expected 431, got {head:?}");
        }
        Ok(Err(err)) => panic!("connection was reset instead of closing with FIN: {err}"),
        Err(_) => panic!("no response and no close within timeout"),
    }
}
