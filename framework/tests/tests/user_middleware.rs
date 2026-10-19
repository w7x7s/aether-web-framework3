//! Tests for user-defined and group-scoped middleware.

use aether::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn middleware_modifies_response() {
    let mut app = AetherApp::new();
    app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Processed", "yes".to_string()));
        res
    });
    app.get("/", |_| "ok");

    let client = TestClient::start(app).await;
    let res = client.get("/").send().await;
    assert_eq!(res.status, 200);
    assert_eq!(res.header("x-processed").unwrap(), "yes");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_runs_in_registration_order() {
    let log = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let mut app = AetherApp::new();
    app.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("mw1-in");
                let res = next(req).await;
                log.lock().unwrap().push("mw1-out");
                res
            }
        }
    });
    app.use_middleware({
        let log = Arc::clone(&log);
        move |req: RequestContext, next: Next| {
            let log = Arc::clone(&log);
            async move {
                log.lock().unwrap().push("mw2-in");
                let res = next(req).await;
                log.lock().unwrap().push("mw2-out");
                res
            }
        }
    });
    app.get("/", |_| "ok");

    let client = TestClient::start(app).await;
    client.get("/").send().await;
    client.shutdown().await;

    let seen: Vec<&'static str> = log.lock().unwrap().clone();
    assert_eq!(seen, ["mw1-in", "mw2-in", "mw2-out", "mw1-out"]);
}

#[tokio::test]
async fn middleware_can_short_circuit() {
    static HANDLER_HITS: AtomicUsize = AtomicUsize::new(0);
    let mut app = AetherApp::new();
    app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.header("x-block").is_some() {
            return Response::status(403, "Forbidden");
        }
        next(req).await
    });
    app.get("/", |_| {
        HANDLER_HITS.fetch_add(1, Ordering::SeqCst);
        "ok"
    });

    let client = TestClient::start(app).await;
    let blocked = client.get("/").header("X-Block", "1").send().await;
    assert_eq!(blocked.status, 403);
    let allowed = client.get("/").send().await;
    assert_eq!(allowed.status, 200);
    assert_eq!(HANDLER_HITS.load(Ordering::SeqCst), 1);
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_applies_only_to_routes_registered_after() {
    let mut app = AetherApp::new();
    app.get("/before", |_| "untouched");
    app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Mw", "1".to_string()));
        res
    });
    app.get("/after", |_| "touched");

    let client = TestClient::start(app).await;
    let before = client.get("/before").send().await;
    let after = client.get("/after").send().await;
    assert!(before.header("x-mw").is_none());
    assert_eq!(after.header("x-mw").unwrap(), "1");
    client.shutdown().await;
}

#[tokio::test]
async fn group_middleware_scopes_to_group() {
    let mut app = AetherApp::new();
    app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-App", "1".to_string()));
        res
    });
    app.group("/admin", |g| {
        g.use_middleware(|req: RequestContext, next: Next| async move {
            let mut res = next(req).await;
            res.headers.push(("X-Admin", "1".to_string()));
            res
        });
        g.get("/panel", |_| "admin panel");
        g.get_async("/stats", |req: RequestContext| async move {
            let _ = req;
            "stats"
        });
    });
    app.get("/public", |_| "public");

    let client = TestClient::start(app).await;
    let admin = client.get("/admin/panel").send().await;
    assert_eq!(admin.header("x-app").unwrap(), "1");
    assert_eq!(admin.header("x-admin").unwrap(), "1");

    let public = client.get("/public").send().await;
    assert_eq!(public.header("x-app").unwrap(), "1");
    assert!(public.header("x-admin").is_none());

    let admin_async = client.get("/admin/stats").send().await;
    assert_eq!(admin_async.header("x-admin").unwrap(), "1");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_observes_and_can_modify_request() {
    let mut app = AetherApp::new();
    app.use_middleware(|mut req: RequestContext, next: Next| async move {
        req.request_id.clear();
        req.request_id.push_str("injected");
        next(req).await
    });
    app.get("/", |req: RequestContext| req.request_id().to_string());

    let client = TestClient::start(app).await;
    let res = client.get("/").send().await;
    assert_eq!(res.text(), "injected");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_error_does_not_break_server() {
    let mut app = AetherApp::new();
    app.use_middleware(|req: RequestContext, next: Next| async move {
        if req.path == "/fail" {
            panic!("middleware exploded");
        }
        next(req).await
    });
    app.get("/fail", |_| "never reached");
    app.get("/ok", |_| "still alive");

    let client = TestClient::start(app).await;
    let failed = client.get("/fail").send().await;
    assert_eq!(failed.status, 500);
    let ok = client.get("/ok").send().await;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.text(), "still alive");
    client.shutdown().await;
}

#[tokio::test]
async fn middleware_composes_with_extractors() {
    let mut app = AetherApp::new();
    app.use_middleware(|req: RequestContext, next: Next| async move {
        let mut res = next(req).await;
        res.headers.push(("X-Mw", "on".to_string()));
        res
    });
    app.get_async("/users/{id}", |Path(id): Path<u32>| async move {
        format!("user {id}")
    });

    let client = TestClient::start(app).await;
    let res = client.get("/users/5").send().await;
    assert_eq!(res.text(), "user 5");
    assert_eq!(res.header("x-mw").unwrap(), "on");
    client.shutdown().await;
}
