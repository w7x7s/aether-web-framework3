//! Tests for `aether::test_client::TestClient`.
//!
//! The client drives a real server over TCP, so these tests verify both the client itself
//! and that the public testing API exercises the full request path.

use aether::prelude::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn echo_app() -> AetherApp {
    let mut app = AetherApp::new();
    app.get("/hello", |_| "hello");
    app.get("/users/{id}", |req| {
        let id = req.param("id").unwrap_or("?").to_string();
        let q = req.query_decoded("q").unwrap_or_default();
        format!("id={id} q={q}")
    });
    app.post("/echo", |req| {
        let body: Value = req.json().expect("json body");
        Response::json(bytes::Bytes::from(body.to_string()))
    });
    app.post("/form", |req| {
        let a = req.form_data().get("a").cloned().unwrap_or_default();
        let b = req.form_data().get("b").cloned().unwrap_or_default();
        format!("{a}:{b}")
    });
    app.get("/whoami", |req| {
        req.header("x-custom").unwrap_or("none").to_string()
    });
    app.get("/state", |req| {
        let counter = req.state::<Arc<AtomicUsize>>().expect("managed state");
        let n = counter.fetch_add(1, Ordering::SeqCst);
        format!("count={n}")
    });
    app
}

#[tokio::test]
async fn get_text_roundtrip() {
    let client = TestClient::start(echo_app()).await;
    let res = client.get("/hello").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "hello");
    assert!(res.is_success());
    assert_eq!(res.header("content-type").unwrap(), "text/plain; charset=utf-8");
    client.shutdown().await;
}

#[tokio::test]
async fn url_points_at_bound_port() {
    let client = TestClient::start(echo_app()).await;
    assert!(client.url("/hello").starts_with("http://127.0.0.1:"));
    assert!(client.addr().port() > 0);
    client.shutdown().await;
}

#[tokio::test]
async fn path_params_and_decoded_query() {
    let client = TestClient::start(echo_app()).await;
    let res = client.get("/users/42?q=%D9%85%D8%AD%D9%85%D8%AF").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "id=42 q=محمد");
    client.shutdown().await;
}

#[tokio::test]
async fn post_json_body_and_json_helper() {
    let client = TestClient::start(echo_app()).await;
    let res = client
        .post("/echo")
        .json(&json!({"a": 1, "b": "two"}))
        .expect("serialize")
        .send()
        .await;
    assert_eq!(res.status, 200);
    let body: Value = res.json().expect("response json");
    assert_eq!(body, json!({"a": 1, "b": "two"}));
    client.shutdown().await;
}

#[tokio::test]
async fn form_post_is_decoded_by_handler() {
    let client = TestClient::start(echo_app()).await;
    let res = client
        .post("/form")
        .form(&[("a", "x y"), ("b", "ز")])
        .send()
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "x y:ز");
    client.shutdown().await;
}

#[tokio::test]
async fn custom_request_header_reaches_handler() {
    let client = TestClient::start(echo_app()).await;
    let res = client.get("/whoami").header("X-Custom", "abc").send().await;
    assert_eq!(res.text(), "abc");
    client.shutdown().await;
}

#[tokio::test]
async fn managed_state_is_shared() {
    let mut app = echo_app();
    app.manage(Arc::new(AtomicUsize::new(0)));
    let client = TestClient::start(app).await;
    let first = client.get("/state").send().await;
    let second = client.get("/state").send().await;
    assert_eq!(first.text(), "count=0");
    assert_eq!(second.text(), "count=1");
    client.shutdown().await;
}

#[tokio::test]
async fn unknown_path_is_404() {
    let client = TestClient::start(echo_app()).await;
    let res = client.get("/nope").send().await;
    assert_eq!(res.status, 404);
    assert!(!res.is_success());
    client.shutdown().await;
}

#[tokio::test]
async fn head_carries_length_without_body() {
    let mut app = AetherApp::new();
    app.get("/thing", |_| "payload");
    let client = TestClient::start(app).await;

    let head = client.head("/thing").send().await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("content-length").unwrap(), "7");
    assert!(head.body.is_empty());

    let get = client.get("/thing").send().await;
    assert_eq!(get.text(), "payload");
    client.shutdown().await;
}

#[tokio::test]
async fn security_headers_applied_when_enabled() {
    let mut app = AetherApp::new();
    app.enable_security_headers();
    app.get("/", |_| "ok");
    let client = TestClient::start(app).await;
    let res = client.get("/").send().await;
    assert!(res.header("strict-transport-security").is_some());
    assert!(res.header("x-frame-options").is_some());
    client.shutdown().await;
}

#[tokio::test]
async fn request_id_header_returned() {
    let client = TestClient::start(echo_app()).await;
    let res = client.get("/hello").send().await;
    let id = res.header("x-request-id").expect("request id header");
    assert!(id.starts_with("req_"));
    client.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_accepting_connections() {
    let addr = {
        let client = TestClient::start(echo_app()).await;
        let addr = client.addr();
        let res = client.get("/hello").send().await;
        assert_eq!(res.status, 200);
        addr
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let connect = tokio::net::TcpStream::connect(addr).await;
    assert!(connect.is_err(), "server still accepting after shutdown");
}
