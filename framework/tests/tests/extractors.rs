//! Tests for the typed extractor system (`Path`, `Query`, `Json`, `Form`, `State`).

use aether::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Deserialize, PartialEq)]
struct UserPath {
    id: u32,
    tab: String,
}

#[derive(Debug, Deserialize, PartialEq)]
struct PageQuery {
    page: u32,
    filter: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct CreateUser {
    name: String,
    age: u8,
}

#[derive(Debug, Deserialize, PartialEq)]
struct LoginForm {
    username: String,
    remember: Option<bool>,
}

struct Hits(Arc<AtomicUsize>);

fn extractor_app() -> AetherApp {
    let mut app = AetherApp::new();
    app.manage(Hits(Arc::new(AtomicUsize::new(0))));

    app.get_async("/users/{id}", |Path(id): Path<u32>| async move {
        format!("user {id}")
    });
    app.get_async("/named/{name}", |Path(name): Path<String>| async move {
        format!("hello {name}")
    });
    app.get_async("/users/{id}/tabs/{tab}", |Path(p): Path<UserPath>| async move {
        format!("{}@{}", p.id, p.tab)
    });
    app.get_async("/search", |Query(q): Query<PageQuery>| async move {
        let filter = q.filter.unwrap_or_else(|| "none".to_string());
        format!("page={} filter={filter}", q.page)
    });
    app.post_async("/json", |Json(body): Json<CreateUser>| async move {
        format!("{} is {}", body.name, body.age)
    });
    app.post_async("/form", |Form(f): Form<LoginForm>| async move {
        let remember = f.remember.unwrap_or(false);
        format!("{} remember={remember}", f.username)
    });
    app.get_async("/hits", |State(h): State<Hits>| async move {
        let n = h.0.fetch_add(1, Ordering::SeqCst);
        format!("hit {n}")
    });
    app.get_async("/multi/{id}", |Path(id): Path<u32>, Query(q): Query<PageQuery>| async move {
        format!("{id}:{}", q.page)
    });
    app.get_async("/ctx", |req: RequestContext| async move {
        req.header("x-trace").unwrap_or("no-trace").to_string()
    });
    app.get_async("/zero", || async { "zero args" });
    app
}

#[tokio::test]
async fn path_primitive() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/users/42").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "user 42");
    client.shutdown().await;
}

#[tokio::test]
async fn path_string() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/named/sara%20ahmad").send().await;
    assert_eq!(res.text(), "hello sara ahmad");
    client.shutdown().await;
}

#[tokio::test]
async fn path_struct_with_multiple_params() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/users/7/tabs/inbox").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "7@inbox");
    client.shutdown().await;
}

#[tokio::test]
async fn path_rejects_non_numeric_primitive() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/users/notanumber").send().await;
    assert_eq!(res.status, 400);
    let body: Value = res.json().expect("json error body");
    assert!(body["error"].as_str().unwrap().contains("invalid path parameters"));
    client.shutdown().await;
}

#[tokio::test]
async fn query_struct_with_optional_field() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/search?page=3").send().await;
    assert_eq!(res.text(), "page=3 filter=none");
    let res = client.get("/search?page=3&filter=active%20users").send().await;
    assert_eq!(res.text(), "page=3 filter=active users");
    client.shutdown().await;
}

#[tokio::test]
async fn query_rejects_missing_required_field() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/search?filter=x").send().await;
    assert_eq!(res.status, 400);
    client.shutdown().await;
}

#[tokio::test]
async fn json_body_roundtrip() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/json")
        .json(&json!({"name": "Zaid", "age": 30}))
        .expect("serialize")
        .send()
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "Zaid is 30");
    client.shutdown().await;
}

#[tokio::test]
async fn json_rejects_wrong_content_type_with_415() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/json")
        .header("Content-Type", "text/plain")
        .body("name=Zaid")
        .send()
        .await;
    assert_eq!(res.status, 415);
    client.shutdown().await;
}

#[tokio::test]
async fn json_rejects_malformed_body_with_400() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/json")
        .header("Content-Type", "application/json")
        .body("{not json")
        .send()
        .await;
    assert_eq!(res.status, 400);
    let body: Value = res.json().expect("json error body");
    assert!(body["error"].as_str().unwrap().contains("invalid JSON body"));
    client.shutdown().await;
}

#[tokio::test]
async fn form_body_with_optional_flag() {
    let client = TestClient::start(extractor_app()).await;
    let res = client
        .post("/form")
        .form(&[("username", "omar"), ("remember", "true")])
        .send()
        .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "omar remember=true");
    client.shutdown().await;
}

#[tokio::test]
async fn state_extractor_reads_managed_state() {
    let client = TestClient::start(extractor_app()).await;
    let first = client.get("/hits").send().await;
    let second = client.get("/hits").send().await;
    assert_eq!(first.text(), "hit 0");
    assert_eq!(second.text(), "hit 1");
    client.shutdown().await;
}

#[tokio::test]
async fn multi_extractor_handler() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/multi/9?page=5").send().await;
    assert_eq!(res.text(), "9:5");
    client.shutdown().await;
}

#[tokio::test]
async fn request_context_extractor_still_passes_by_move() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/ctx").header("X-Trace", "abc123").send().await;
    assert_eq!(res.text(), "abc123");
    let res = client.get("/ctx").send().await;
    assert_eq!(res.text(), "no-trace");
    client.shutdown().await;
}

#[tokio::test]
async fn zero_argument_handler() {
    let client = TestClient::start(extractor_app()).await;
    let res = client.get("/zero").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "zero args");
    client.shutdown().await;
}

#[tokio::test]
async fn extractors_work_in_route_groups() {
    let mut app = AetherApp::new();
    app.group("/api/v1", |g| {
        g.get_async("/items/{id}", |Path(id): Path<u64>| async move {
            format!("item {id}")
        });
    });
    let client = TestClient::start(app).await;
    let res = client.get("/api/v1/items/99").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.text(), "item 99");
    client.shutdown().await;
}

#[tokio::test]
async fn state_extractor_missing_type_is_500() {
    struct NeverManaged;
    let mut app = AetherApp::new();
    app.get_async("/oops", |_: State<NeverManaged>| async { "unreachable" });
    let client = TestClient::start(app).await;
    let res = client.get("/oops").send().await;
    assert_eq!(res.status, 500);
    let body: Value = res.json().expect("json error body");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("managed state not found"));
    client.shutdown().await;
}

#[tokio::test]
async fn extraction_failure_short_circuits_handler() {
    static CALLED: AtomicUsize = AtomicUsize::new(0);
    let mut app = AetherApp::new();
    app.post_async("/guarded", |Json(_): Json<CreateUser>| async {
        CALLED.fetch_add(1, Ordering::SeqCst);
        "ran"
    });
    let client = TestClient::start(app).await;
    let res = client
        .post("/guarded")
        .header("Content-Type", "application/json")
        .body("[]")
        .send()
        .await;
    assert_eq!(res.status, 400);
    assert_eq!(CALLED.load(Ordering::SeqCst), 0, "handler must not run on rejection");
    client.shutdown().await;
}
