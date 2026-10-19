//! Stage 0: wire-level contract tests for handler EXECUTION semantics.
//!
//! These tests pin how the serve path runs handlers today — spawn + timeout,
//! abort-vs-completion, panic isolation, disconnect behaviour, and drain
//! behaviour — as observable HTTP/wire facts. They are the behavioural
//! contract any execution-path change (e.g. an inline fast path) must keep
//! passing. Where the current behaviour is itself a documented compromise
//! (blocking sync handlers are never preempted; disconnect does not cancel a
//! running handler; drain aborts the connection but its spawned handler task
//! finishes as an orphan), the test records that compromise explicitly.

use aether::prelude::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
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

    fn signal_shutdown(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }

    async fn finish_shutdown(self) {
        match tokio::time::timeout(Duration::from_secs(10), self.task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => panic!("server returned an error: {error}"),
            Ok(Err(join)) => panic!("server task failed: {join}"),
            Err(_) => panic!("server did not finish shutdown within 10s"),
        }
    }

    async fn stop(mut self) {
        self.signal_shutdown();
        self.finish_shutdown().await;
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Buffered raw client: body bytes that arrive coalesced with the response
/// head stay in `buffered`, so keep-alive follow-up reads never block on a
/// socket the server has already answered.
struct Client {
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).await.expect("connect to test server");
        stream.set_nodelay(true).expect("TCP_NODELAY");
        Self {
            stream,
            buffered: Vec::new(),
        }
    }

    async fn get(&mut self, path: &str) {
        let bytes = format!("GET {path} HTTP/1.1\r\nHost: test\r\n\r\n").into_bytes();
        self.stream.write_all(&bytes).await.expect("write request");
        self.stream.flush().await.expect("flush");
    }

    fn take_response(&mut self) -> Option<HttpResponse> {
        let header_end = find_subslice(&self.buffered, b"\r\n\r\n")?;
        let header_text = String::from_utf8_lossy(&self.buffered[..header_end]).into_owned();
        let mut lines = header_text.split("\r\n");
        let status: u16 = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))?
            .parse()
            .ok()?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        let body_len = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let total = header_end + 4 + body_len;
        if self.buffered.len() < total {
            return None;
        }
        self.buffered.drain(..total);
        Some(HttpResponse { status, headers })
    }

    async fn response(&mut self) -> HttpResponse {
        loop {
            if let Some(response) = self.take_response() {
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

    async fn expect_closed(&mut self) {
        let mut chunk = [0u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk)).await {
                Ok(Ok(0)) | Ok(Err(_)) => return,
                Ok(Ok(_)) => continue,
                Err(_) => panic!("server kept the connection open where closure was expected"),
            }
        }
    }

    /// Shutdown consumed the pending response bytes: the connection was
    /// aborted mid-request, so the client must see the socket close (or the
    /// already-received head) without the server ever hanging it.
    async fn closed_or_answered(&mut self) {
        let mut chunk = [0u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(5), self.stream.read(&mut chunk)).await {
                Ok(Ok(0)) | Ok(Err(_)) => return,
                Ok(Ok(_)) => continue,
                Err(_) => panic!("connection stayed open with no response after drain"),
            }
        }
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// CONTRACT: a panicking SYNC handler becomes a 500 on the wire and the
/// keep-alive connection keeps serving subsequent requests. Panic isolation
/// must not depend on adding another mechanism later.
#[tokio::test]
async fn sync_handler_panic_returns_500_and_keepalive_survives() {
    let mut app = AetherApp::new();
    app.get("/ok", |_| "ok");
    app.get("/panic", |_| -> &'static str { panic!("intentional contract panic") });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.get("/panic").await;
    assert_eq!(client.response().await.status, 500);

    client.get("/ok").await;
    assert_eq!(client.response().await.status, 200);

    server.stop().await;
}

/// CONTRACT: a panicking ASYNC handler (panic after an await point) also
/// becomes a 500 and the connection survives — the isolation is not limited
/// to the sync-wrapped path.
#[tokio::test]
async fn async_handler_panic_returns_500_and_keepalive_survives() {
    let mut app = AetherApp::new();
    app.get("/ok", |_| "ok");
    app.get_async("/boom", |_: RequestContext| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        panic!("intentional async contract panic");
        #[allow(unreachable_code)]
        "never"
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;

    client.get("/boom").await;
    assert_eq!(client.response().await.status, 500);

    client.get("/ok").await;
    assert_eq!(client.response().await.status, 200);

    server.stop().await;
}

/// CONTRACT: an async handler that exceeds `max_request_time` is genuinely
/// ABORTED: the client gets 504 + `Connection: close`, the socket is closed,
/// and the handler's post-sleep side effect never runs — the task is
/// cancelled at its await point, not merely ignored.
#[tokio::test]
async fn async_handler_over_timeout_is_aborted_with_504_and_close() {
    let mut config = AetherConfig::default();
    config.max_request_time = Duration::from_millis(150);
    let mut app = AetherApp::with_config(config);

    let finished = Arc::new(AtomicBool::new(false));
    let finished_probe = Arc::clone(&finished);
    app.get_async("/hang", move |_: RequestContext| {
        let finished_probe = Arc::clone(&finished_probe);
        async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            finished_probe.store(true, Ordering::SeqCst);
            Response::text("late")
        }
    });

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client.get("/hang").await;

    let response = client.response().await;
    assert_eq!(response.status, 504);
    assert_eq!(
        response.header("Connection"),
        Some("close"),
        "the timeout response must instruct the client to abandon the socket"
    );
    client.expect_closed().await;

    // The handler's sleep would have completed at t≈500ms; the abort must
    // prevent the side effect entirely.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        !finished.load(Ordering::SeqCst),
        "documented current behaviour: async handlers ARE cancelled at the request timeout"
    );

    server.stop().await;
}

/// CONTRACT: a SYNC handler that blocks the worker past `max_request_time`
/// is NEVER preempted, and the wire stays coherent either way. Which answer
/// the client gets is race-dependent in the current build — verified, not
/// assumed: the handler task is spawned into the connection task's own
/// worker slot, and while that worker blocks inside the handler poll it
/// cannot process ITS timer shard, so the 504 may only arm after the handler
/// already returned (late 200). When another worker owns the armed sleep,
/// the client sees 504 + `Connection: close`. Both are legal; neither may
/// desync or hang the connection, and the blocking handler always runs to
/// completion (an abort cannot interrupt an in-flight poll).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocking_sync_handler_timeout_is_best_effort_and_never_preempted() {
    let mut config = AetherConfig::default();
    config.max_request_time = Duration::from_millis(150);
    let mut app = AetherApp::with_config(config);

    let completed = Arc::new(AtomicBool::new(false));
    let completed_handler = Arc::clone(&completed);
    app.get("/block", move |_| {
        std::thread::sleep(Duration::from_millis(450));
        completed_handler.store(true, Ordering::SeqCst);
        "blocked"
    });
    app.get("/ok", |_| "ok");

    let server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    let started = Instant::now();
    client.get("/block").await;

    let response = client.response().await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the wire must reach a terminal response; took {:?}",
        started.elapsed()
    );
    match response.status {
        504 => {
            assert_eq!(
                response.header("Connection"),
                Some("close"),
                "a 504 must instruct the client to abandon the socket"
            );
            client.expect_closed().await;
        }
        200 => {
            // Late-but-complete: the blocked worker could not arm its timer
            // shard in time. The keep-alive connection must still be sound.
            client.get("/ok").await;
            assert_eq!(client.response().await.status, 200);
        }
        other => panic!("blocking handler must produce 200 or 504, got {other}"),
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        completed.load(Ordering::SeqCst),
        "documented current behaviour: the blocking handler is NOT cancelled; it always runs to completion"
    );

    server.stop().await;
}

/// CONTRACT: a client disconnect does NOT cancel an already-running handler:
/// its side effect completes, the failed write only tears down that one
/// connection, and the server keeps serving new connections.
#[tokio::test]
async fn client_disconnect_does_not_cancel_running_handler() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_handler = Arc::clone(&hits);
    let mut app = AetherApp::new();
    app.get_async("/slow", move |_: RequestContext| {
        let hits = Arc::clone(&hits_handler);
        async move {
            tokio::time::sleep(Duration::from_millis(350)).await;
            hits.fetch_add(1, Ordering::SeqCst);
            Response::text("late")
        }
    });
    app.get("/ok", |_| "ok");

    let server = TestServer::start(app).await;

    let mut client = Client::connect(server.addr).await;
    client.get("/slow").await;
    drop(client); // abrupt disconnect while the handler is mid-flight

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "documented current behaviour: disconnect does not cancel the handler task"
    );

    let mut fresh = Client::connect(server.addr).await;
    fresh.get("/ok").await;
    assert_eq!(fresh.response().await.status, 200);

    server.stop().await;
}

/// CONTRACT: graceful shutdown with a handler still running past the drain
/// budget aborts the CONNECTION task (client sees the socket close, no
/// response), while the spawned handler task keeps running as an orphan and
/// completes after `serve_with_shutdown` has already returned.
#[tokio::test]
async fn drain_timeout_aborts_connection_but_handler_finishes_orphaned() {
    let mut config = AetherConfig::default();
    // Drain budget is keep_alive_timeout.min(30s); shrink it so the abort fires
    // while the handler is still mid-flight.
    config.keep_alive_timeout = Duration::from_millis(200);
    let mut app = AetherApp::with_config(config);

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_handler = Arc::clone(&hits);
    app.get_async("/slow", move |_: RequestContext| {
        let hits = Arc::clone(&hits_handler);
        async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            hits.fetch_add(1, Ordering::SeqCst);
            Response::text("late")
        }
    });

    let mut server = TestServer::start(app).await;
    let mut client = Client::connect(server.addr).await;
    client.get("/slow").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let started = Instant::now();
    server.signal_shutdown();
    server.finish_shutdown().await;
    let shutdown_elapsed = started.elapsed();
    assert!(
        shutdown_elapsed < Duration::from_millis(500),
        "shutdown waited {:?} for the connection; the drain budget (200ms) was supposed to bound it",
        shutdown_elapsed
    );

    // The aborted connection closes the socket without ever producing a response.
    client.closed_or_answered().await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "handler should still be mid-sleep when serve() returned"
    );

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "documented current behaviour: the handler task survives the drain as an orphan"
    );
}

/// Stage-B gate measurement: blast radius of ONE blocking sync handler
/// (450ms) on (a) the next request on the SAME keep-alive connection and
/// (b) a request on an INDEPENDENT connection started mid-block. Latencies
/// are recorded (printed with --nocapture) as the current-path reference
/// numbers; the hard assertions are the invariants any execution change
/// must keep: both responses complete, ordering holds, no desync, and the
/// same-connection follow-up is necessarily serialized behind the block.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocking_sync_handler_blast_radius_is_measured() {
    let mut app = AetherApp::new();
    app.get("/block", |_| {
        std::thread::sleep(Duration::from_millis(450));
        "blocked"
    });
    app.get("/quick", |_| "quick");

    let server = TestServer::start(app).await;

    // (a) Same connection, pipelined: /quick already on the wire while /block
    // is mid-handler. The server must answer in order, and /quick's response
    // may only arrive after the block completes — that wait is the visible
    // blast radius on the same connection.
    let mut same = Client::connect(server.addr).await;
    same.get("/block").await;
    let t = Instant::now();
    same.get("/quick").await; // not awaited yet: sits in the connection buffer
    let blocked = same.response().await;
    let block_latency = t.elapsed();
    let quick_after = same.response().await;
    let same_conn_followup = t.elapsed(); // /quick end-to-end behind the block

    // (b) Independent victim connection issued 50ms into a fresh block.
    let mut blocker = Client::connect(server.addr).await;
    blocker.get("/block").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut victim = Client::connect(server.addr).await;
    let t = Instant::now();
    victim.get("/quick").await;
    let victim_response = victim.response().await;
    let independent_latency = t.elapsed();
    let blocker_response = blocker.response().await;

    println!(
        "BLAST-RADIUS block={block_latency:?} same_conn_followup={same_conn_followup:?} \
         independent_victim={independent_latency:?}"
    );

    assert_eq!(blocked.status, 200);
    assert_eq!(quick_after.status, 200);
    assert_eq!(victim_response.status, 200);
    assert_eq!(blocker_response.status, 200);
    // The follow-up on the blocked connection CANNOT beat the block: the
    // connection is request-serialized. This is the serialization floor.
    assert!(
        same_conn_followup >= Duration::from_millis(350),
        "same-connection follow-up answered in {same_conn_followup:?} — behind a 450ms block, \
         which would mean responses crossed"
    );
    // The independent victim must complete no matter what (it may be delayed,
    // which the recorded number exposes, but never starved to failure).
    assert!(
        independent_latency < Duration::from_secs(2),
        "independent victim took {independent_latency:?}: starvation, not just delay"
    );

    server.stop().await;
}
