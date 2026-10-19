# 08. دليل البدء السريع وأمثلة الاستخدام (Getting Started & Examples)

يوضح هذا الدليل كيفية إنشاء وتوسيع تطبيق باستخدام إطار عمل Aether، مع أمثلة للتعامل مع الـ JSON، المسارات الديناميكية، الـ State المشتركة، وحماية الـ Endpoints.

---

## 1. تطبيق بسيط (Hello World & JSON API)

```rust
use aether::app::AetherApp;
use aether::context::RequestContext;
use http::response::Response;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct User {
    id: u32,
    name: String,
}

#[derive(Deserialize)]
struct CreateUserRequest {
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = AetherApp::new();

    // 1. مسار نصي بسيط
    app.get("/", |_ctx: RequestContext| async move {
        Response::ok("Hello from Aether Fast Framework! ⚡")
    });

    // 2. مسار ديناميكي مع معلمة مسار (Path Parameter)
    app.get("/users/:id", |ctx: RequestContext| async move {
        let user_id = ctx.param("id").unwrap_or("0");
        let response_data = User {
            id: user_id.parse().unwrap_or(0),
            name: "John Doe".to_string(),
        };
        Response::json(&response_data)
    });

    // 3. مسار POST لاستقبال وفك تشفير JSON
    app.post("/users", |ctx: RequestContext| async move {
        match ctx.json::<CreateUserRequest>() {
            Ok(payload) => {
                let new_user = User {
                    id: 101,
                    name: payload.name,
                };
                Response::status(201, "Created")
                    .with_json(&new_user)
            }
            Err(_) => Response::status(400, "Bad Request: Invalid JSON"),
        }
    });

    println!("🚀 Aether Server listening on http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await?;

    Ok(())
}
```

---

## 2. إدارة الحالة المشتركة (Shared State)

```rust
use aether::app::AetherApp;
use aether::context::RequestContext;
use http::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

struct AppState {
    counter: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(AppState {
        counter: AtomicU64::new(0),
    });

    let mut app = AetherApp::with_state(state);

    app.get("/hits", |ctx: RequestContext| async move {
        // الوصول للحالة المشتركة بأمان فائق دون أقفال ثقيلة
        let hits = ctx.state.counter.fetch_add(1, Ordering::Relaxed) + 1;
        Response::ok(format!("Total Hits: {}", hits))
    });

    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```

---

## 3. تخصيص حدود الأمان والـ Timeouts

```rust
use aether::app::AetherApp;
use http::limits::ServerLimits;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let custom_limits = ServerLimits {
        max_header_bytes: 16 * 1024,          // 16 KB
        max_body_bytes: 10 * 1024 * 1024,     // 10 MB
        max_headers_count: 100,
        max_connections: 20_000,              // دعم حتى 20 ألف اتصال متزامن
        read_timeout: Duration::from_secs(3),
        write_timeout: Duration::from_secs(5),
        keep_alive_timeout: Duration::from_secs(30),
    };

    let mut app = AetherApp::with_limits(custom_limits);
    // تسجيل المسارات...
    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```
