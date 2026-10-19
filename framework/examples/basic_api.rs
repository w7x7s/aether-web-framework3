use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 1. الصفحة الرئيسية HTML
    app.get("/", |_req| {
        Response::html(
            "<!DOCTYPE html><html lang='ar' dir='rtl'><head><meta charset='utf-8'><title>Aether Framework</title><style>body{background:#0f172a;color:#fff;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}.box{background:#1e293b;padding:40px;border-radius:12px;text-align:center;box-shadow:0 10px 30px rgba(0,0,0,0.5)}h1{color:#38bdf8;margin-bottom:10px}a{display:inline-block;margin:10px;padding:10px 20px;background:#0284c7;color:#fff;text-decoration:none;border-radius:6px}</style></head><body><div class='box'><h1>⚡ مرحباً بك في Aether Framework!</h1><p>تم بناؤه بأبسط كود ممكن في سطرين فقط!</p><a href='/health'>Health Check</a><a href='/api/users'>قائمة المستخدمين</a><a href='/users/hamza'>المستخدم حمزة</a></div></body></html>"
        )
    });

    // 2. مسار فحص الـ Health
    app.get("/health", |_req| {
        Response::json(bytes::Bytes::from(r#"{"status":"healthy","engine":"Aether Rust"}"#))
    });

    // 3. مسار API للمستخدمين
    app.get("/api/users", |_req| {
        Response::json(bytes::Bytes::from(r#"[{"id":1,"name":"حمزة"},{"id":2,"name":"سارة"}]"#))
    });

    // 4. مسار مع Dynamic Route Parameter (:id)
    app.get("/users/{id}", |req| {
        let user_id = req.param("id").unwrap_or("unknown");
        let body = serde_json::json!({
            "user_id": user_id,
            "name": "Hamza",
            "role": "Aether Lead Architect"
        })
        .to_string();
        Response::json(bytes::Bytes::from(body))
    });

    // 5. مسار استقبال بيانات POST مع قراءة الـ Body
    app.post("/api/items", |req| {
        let body_content = req.body_str().unwrap_or("{}");
        // Parse valid JSON so it can be embedded as a value; anything else is echoed as an
        // escaped string. Interpolating the raw body into a JSON template would let a
        // caller inject or corrupt the response.
        let received = serde_json::from_str::<serde_json::Value>(body_content.trim())
            .unwrap_or_else(|_| serde_json::Value::String(body_content.to_string()));
        let body = serde_json::json!({
            "success": true,
            "received_body": received,
            "message": "تم استقبال ومعالجة البيانات بنجاح"
        })
        .to_string();
        Response::json(bytes::Bytes::from(body))
    });

    // تشغيل الخادم
    app.listen("0.0.0.0:3000").await
}
