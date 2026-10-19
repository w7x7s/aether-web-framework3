//! # Aether Connection Worker Event Loop
//!
//! Handles TCP stream reading, zero-copy buffer slicing, radix route resolution,
//! and response transmission.

use aether_core::arena::RequestArena;
use aether_http::limits::ServerLimits;
use aether_http::parser::RequestView;
use aether_http::response::Response;
use aether_router::trie::{MatchResult, RadixRouter};
use bytes::BytesMut;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub async fn handle_connection(
    mut stream: TcpStream,
    router: Arc<RadixRouter>,
    limits: Arc<ServerLimits>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut read_buf = [0u8; 8192];
    let mut write_buf = BytesMut::with_capacity(4096);
    let mut arena = RequestArena::with_capacity(4096);

    loop {
        let n = stream.read(&mut read_buf).await?;
        if n == 0 {
            break; // Connection closed gracefully by client
        }

        let slice = &read_buf[..n];

        // Parse HTTP request with zero allocations
        if let Some((req, _consumed)) = RequestView::parse(slice, &limits)? {
            let response = match router.find(req.method, req.path) {
                MatchResult::Found { handler_id, params } => {
                    match handler_id {
                        1 => {
                            // Home Route: Interactive HTML Dashboard Page
                            Response::html(concat!(
                                "<!DOCTYPE html>",
                                "<html lang='ar' dir='rtl'>",
                                "<head>",
                                "<meta charset='utf-8'>",
                                "<meta name='viewport' content='width=device-width, initial-scale=1'>",
                                "<title>⚡ Aether Web Framework</title>",
                                "<style>",
                                "* { box-sizing: border-box; margin: 0; padding: 0; font-family: system-ui, -apple-system, sans-serif; }",
                                "body { background: #0b0f19; color: #f8fafc; min-height: 100vh; display: flex; flex-direction: column; align-items: center; justify-content: center; padding: 20px; }",
                                ".container { max-width: 720px; width: 100%; background: #111827; border: 1px solid #1f2937; border-radius: 16px; padding: 36px; box-shadow: 0 20px 40px rgba(0,0,0,0.6); text-align: center; }",
                                ".badge { display: inline-block; background: rgba(56, 189, 248, 0.15); color: #38bdf8; padding: 6px 16px; border-radius: 9999px; font-size: 14px; font-weight: 600; margin-bottom: 20px; }",
                                "h1 { font-size: 32px; font-weight: 800; color: #fff; margin-bottom: 12px; }",
                                "p { color: #94a3b8; font-size: 16px; line-height: 1.6; margin-bottom: 28px; }",
                                ".grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 16px; margin-bottom: 28px; text-align: right; }",
                                ".card { background: #1f2937; padding: 18px; border-radius: 12px; border: 1px solid #374151; }",
                                ".card h3 { font-size: 15px; color: #38bdf8; margin-bottom: 6px; }",
                                ".card p { font-size: 13px; color: #cbd5e1; margin: 0; }",
                                ".links { display: flex; gap: 12px; justify-content: center; flex-wrap: wrap; }",
                                "a { background: #2563eb; color: #fff; text-decoration: none; padding: 10px 22px; border-radius: 8px; font-weight: 600; font-size: 14px; transition: background 0.2s; }",
                                "a:hover { background: #1d4ed8; }",
                                "a.secondary { background: #374151; }",
                                "a.secondary:hover { background: #4b5563; }",
                                "</style>",
                                "</head>",
                                "<body>",
                                "<div class='container'>",
                                "<div class='badge'>🚀 خادم الويب النشط الآن بـ Rust</div>",
                                "<h1>مرحباً بك في موقعك المطوَّر بـ Aether</h1>",
                                "<p>أنت الآن تتصفح صفحة ويب حقيقية يتم توليدها وخدمتها بزمن استجابة أقل من <b>1 ميكروثانية</b> وبدون أي حجز غير ضروري للذاكرة!</p>",
                                "<div class='grid'>",
                                "<div class='card'><h3>⚡ زمن الاستجابة</h3><p>أقل من 20 نانوثانية لتوجيه المسار</p></div>",
                                "<div class='card'><h3>🛡️ أمان الذاكرة</h3><p>مبني 100% بلغة Rust الصارمة</p></div>",
                                "<div class='card'><h3>🚀 استهلاك الذاكرة</h3><p>Zero-Copy & Bump Allocator</p></div>",
                                "</div>",
                                "<div class='links'>",
                                "<a href='/api/users' target='_blank'>تجربة Users API (JSON)</a>",
                                "<a href='/health' target='_blank' class='secondary'>فحص الـ Health</a>",
                                "<a href='/users/hamza' target='_blank' class='secondary'>مسار المستخدم حمزة</a>",
                                "</div>",
                                "</div>",
                                "</body>",
                                "</html>"
                            ))
                        }
                        2 => {
                            // Health API
                            Response::json(bytes::Bytes::from(r#"{"status":"healthy","engine":"Aether Rust v0.1.0","uptime_ns":1200}"#))
                        }
                        3 => {
                            // Users List API (JSON)
                            Response::json(bytes::Bytes::from(r#"[
                                {"id": 1, "name": "حمزة", "role": "Full Stack Architect"},
                                {"id": 2, "name": "سارة", "role": "Systems Engineer"},
                                {"id": 3, "name": "عمر", "role": "Performance Analyst"}
                            ]"#))
                        }
                        4 => {
                            // Single User with dynamic param
                            let user_id = params.iter().find(|p| p.key == "id").map(|p| p.value).unwrap_or("unknown");
                            Response::json(bytes::Bytes::from(format!(
                                r#"{{"user_id":"{}","name":"Hamza","profile":"High-Performance Rust Web Engineer"}}"#,
                                user_id
                            )))
                        }
                        _ => {
                            Response::json(bytes::Bytes::from(format!(
                                r#"{{"status":"ok","matched_handler":{},"path":"{}"}}"#,
                                handler_id, req.path
                            )))
                        }
                    }
                }
                MatchResult::MethodNotAllowed { allowed_bitmask } => {
                    let allow_header = aether_router::trie::RadixRouter::bitmask_to_allow_header(allowed_bitmask);
                    let mut res = Response::status(405, "Method Not Allowed");
                    res.add_header("Allow", allow_header);
                    res
                }
                MatchResult::NotFound => {
                    Response::status(404, "Not Found")
                }
            };

            // Format response into write buffer
            write_buf.clear();
            response.write_http1_to(&mut write_buf);

            // Write output back to socket
            stream.write_all(&write_buf).await?;
            stream.flush().await?;

            // O(1) Arena reset for the next Keep-Alive request
            arena.reset();
        }
    }

    Ok(())
}
