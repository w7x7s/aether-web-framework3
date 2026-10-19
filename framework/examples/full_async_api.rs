use aether::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{sleep, Duration};

/// Global Application State
struct AppState {
    app_name: String,
    request_counter: AtomicU64,
}

/// Strongly Typed JSON Request DTO
#[derive(Deserialize)]
struct CreateUserDto {
    username: String,
    email: String,
    #[allow(dead_code)]
    age: u8,
}

/// Strongly Typed JSON Response DTO
#[derive(Serialize)]
struct UserResponse {
    id: u64,
    username: String,
    email: String,
    status: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 1. تسجيل State عامة للمشروع (Thread-safe Shared State)
    app.manage(AppState {
        app_name: "Aether Next-Gen Cloud API".to_string(),
        request_counter: AtomicU64::new(0),
    });

    // 2. تفعيل الترويسات الأمنية و CORS
    app.enable_security_headers();
    app.enable_cors(CorsMiddleware::new(CorsConfig {
        allow_origin: "*",
        allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
        allow_headers: "Content-Type, Authorization",
        max_age_secs: 86400,
        ..CorsConfig::default()
    }));

    // 3. خدمة ملفات ثابتة (Static File Serving)
    app.static_asset(
        "/static/style.css",
        "text/css; charset=utf-8",
        b"body { font-family: sans-serif; background: #fafafa; }",
    );

    // 4. مسار GET متزامن
    app.get("/", |_req| {
        Response::html("<h1>⚡ مرحبا بك في Aether مع جميع القدرات المتقدمة!</h1>")
    });

    // 5. مسار GET مع قراءة معاملات الاستعلام والـ Cookies
    app.get("/search", |req| {
        let q = req.query_decoded("q").unwrap_or_else(|| "default".to_string());
        let session = req.cookie("session_id").unwrap_or("none");
        let trace = req.request_id();
        let body = serde_json::json!({
            "query": q,
            "session": session,
            "trace_id": trace
        })
        .to_string();
        Response::json(bytes::Bytes::from(body))
    });

    // 6. مسار GET غير متزامن مع .await و State
    app.get_async("/api/async-stat", |req: RequestContext| async move {
        sleep(Duration::from_millis(10)).await;

        if let Some(state) = req.state::<AppState>() {
            let count = state.request_counter.fetch_add(1, Ordering::SeqCst) + 1;
            let body = serde_json::json!({
                "app": state.app_name,
                "total_async_requests": count
            })
            .to_string();
            Response::json(bytes::Bytes::from(body))
        } else {
            Response::status(500, "State not found")
        }
    });

    // 7. مسار POST مع استخراج JSON التلقائي
    app.post_async("/api/users", |req: RequestContext| async move {
        match req.json::<CreateUserDto>() {
            Ok(user) => {
                sleep(Duration::from_millis(5)).await;

                let response_dto = UserResponse {
                    id: 1001,
                    username: user.username,
                    email: user.email,
                    status: "created_successfully".to_string(),
                };

                let json_string = serde_json::to_string(&response_dto).unwrap();
                Response::json(bytes::Bytes::from(json_string))
            }
            Err(_) => Response::status(400, "Bad Request: Invalid JSON Structure"),
        }
    });

    // 8. تجميع المسارات في مجموعات (Route Grouping)
    app.group("/api/v1", |v1| {
        v1.get("/health", |_req| {
            r#"{"status":"healthy","v1":true}"#
        });

        v1.get_async("/users/{id}", |req: RequestContext| async move {
            let id = req.param("id").unwrap_or("0");
            sleep(Duration::from_millis(2)).await;
            let body = serde_json::json!({
                "user_id": id,
                "role": "admin"
            })
            .to_string();
            Response::json(bytes::Bytes::from(body))
        });
    });

    println!("⚡ تشغيل السيرفر على http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await
}
