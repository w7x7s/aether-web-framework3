// Isolated A/B: same socket I/O loop, two handler-execution paths.
//   spawn  = read request -> spawn task -> await with timeout -> write response (mirrors app.rs:1316-1336)
//   inline = read request -> produce response bytes -> write response
// Client is pipelined depth-1 over N keep-alive connections (rawload shape).
// Order rotates each round to cancel machine drift.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PORT: u16 = 3199;
const REQ: &[u8] = b"GET /p HTTP/1.1\r\nHost: x\r\n\r\n";
const RESP: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 13\r\n\r\nHello, World!";

static SERVER_MODE: AtomicU64 = AtomicU64::new(0); // 0=spawn 1=inline

fn done_work() -> &'static [u8] {
    // Same trivial per-request work on both paths.
    let checksum: u32 = REQ.iter().fold(0u32, |a, b| a.wrapping_add(*b as u32));
    if checksum == 0xDEAD {
        return b"";
    }
    RESP
}

async fn serve_conn(mut sock: TcpStream) {
    let mut buf = [0u8; 4096];
    let mut acc = VecDeque::<u8>::new();
    loop {
        // read until a full \r\n\r\n-terminated request is buffered
        loop {
            if window_complete(&acc) {
                break;
            }
            match sock.read(&mut buf).await {
                Ok(0) => return,
                Ok(n) => acc.extend(buf[..n].iter()),
                Err(_) => return,
            }
        }
        while let Some(len) = complete_request_len(&acc) {
            acc.drain(..len);
            let body = done_work();
            if SERVER_MODE.load(Ordering::Relaxed) == 0 {
                let owned = body.to_vec();
                let handle = tokio::spawn(async move {
                    std::hint::black_box(&owned);
                    owned
                });
                let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
                let bytes = match res {
                    Ok(Ok(v)) => v,
                    _ => Vec::new(),
                };
                if sock.write_all(&bytes).await.is_err() {
                    return;
                }
            } else {
                if sock.write_all(body).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn window_complete(buf: &VecDeque<u8>) -> bool {
    complete_request_len(buf).is_some()
}

fn starts_with_crlfcrlf(buf: &VecDeque<u8>, i: usize) -> bool {
    let needle: [u8; 4] = [b'\r', b'\n', b'\r', b'\n'];
    needle.iter().enumerate().all(|(k, b)| buf.get(i + k) == Some(b))
}

fn complete_request_len(buf: &VecDeque<u8>) -> Option<usize> {
    let mut i = 0;
    while i + 4 <= buf.len() {
        if starts_with_crlfcrlf(buf, i) {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}

async fn run_server() -> tokio::task::JoinHandle<()> {
    let listener = TcpListener::bind(("127.0.0.1", PORT)).await.unwrap();
    tokio::spawn(async move {
        loop {
            let (sock, _) = listener.accept().await.unwrap();
            tokio::spawn(serve_conn(sock));
        }
    })
}

struct Stats {
    mean: f64,
    p50: f64,
    p99: f64,
    max: f64,
}

fn report(samples: Vec<u128>) -> Stats {
    let mut v: Vec<u128> = samples;
    v.sort_unstable();
    let n = v.len();
    let sum: u128 = v.iter().sum();
    Stats {
        mean: sum as f64 / n as f64 / 1000.0,
        p50: v[n / 2] as f64 / 1000.0,
        p99: v[(n as f64 * 0.99) as usize] as f64 / 1000.0,
        max: v[n - 1] as f64 / 1000.0,
    }
}

async fn client_round(conns: usize, per_conn: usize) -> Vec<u128> {
    let mut handles = Vec::new();
    for _ in 0..conns {
        handles.push(tokio::spawn(async move {
            let mut sock = TcpStream::connect(("127.0.0.1", PORT)).await.unwrap();
            sock.set_nodelay(true).unwrap();
            let mut buf = [0u8; 4096];
            let mut resp = VecDeque::<u8>::new();
            let mut samples = Vec::with_capacity(per_conn);
            for _ in 0..per_conn {
                let t0 = Instant::now();
                sock.write_all(REQ).await.unwrap();
                while response_complete(&resp).is_none() {
                    let n = sock.read(&mut buf).await.unwrap();
                    if n == 0 {
                        panic!("server closed");
                    }
                    resp.extend(buf[..n].iter());
                }
                let len = response_complete(&resp).unwrap();
                resp.drain(..len);
                samples.push(t0.elapsed().as_nanos());
            }
            samples
        }));
    }
    let mut all = Vec::new();
    for h in handles {
        all.extend(h.await.unwrap());
    }
    all
}

fn response_complete(buf: &VecDeque<u8>) -> Option<usize> {
    // parse content-length, return header_end + cl
    let mut i = 0;
    while i + 4 <= buf.len() {
        if starts_with_crlfcrlf(buf, i) {
            let head_end = i + 4;
            let head: Vec<u8> = buf.iter().take(head_end).cloned().collect();
            let head = String::from_utf8_lossy(&head).to_lowercase();
            let cl = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap();
            return if buf.len() >= head_end + cl {
                Some(head_end + cl)
            } else {
                None
            };
        }
        i += 1;
    }
    None
}

#[tokio::main]
async fn main() {
    let rounds: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let conns: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);
    let per_conn: usize = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000);

    let _server = run_server().await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut spawn_p50 = Vec::new();
    let mut inline_p50 = Vec::new();
    for r in 0..rounds {
        // rotate order
        let order: [(&str, u64); 2] = if r % 2 == 0 {
            [("spawn", 0), ("inline", 1)]
        } else {
            [("inline", 1), ("spawn", 0)]
        };
        for (name, mode) in order {
            SERVER_MODE.store(mode, Ordering::Relaxed);
            // warmup
            let _ = client_round(2, 200).await;
            let s = client_round(conns, per_conn).await;
            let st = report(s);
            println!(
                "round{} {name:6} conns={} p50={:.1}us mean={:.1} p99={:.1} max={:.1}",
                r + 1,
                conns,
                st.p50,
                st.mean,
                st.p99,
                st.max
            );
            if mode == 0 {
                spawn_p50.push(st.p50);
            } else {
                inline_p50.push(st.p50);
            }
        }
    }
    let avg = |v: &Vec<f64>| v.iter().sum::<f64>() / v.len() as f64;
    println!(
        "AVG p50: spawn={:.1}us inline={:.1}us delta={:.1}us (rounds={})",
        avg(&spawn_p50),
        avg(&inline_p50),
        avg(&spawn_p50) - avg(&inline_p50),
        rounds
    );
}
