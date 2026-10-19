# 📚 التوثيق الشامل والرسمي لإطار عمل Aether Web Framework ⚡

> **ملاحظة تدقيق (2026-10-18):** هذا المستند يصف التصميم المقصود، وليس كل ما ورد فيه مُنفَّذاً.
> `aether-http` لا يدعم HTTP/2 (الخادم HTTP/1.1 فقط)؛ `RequestArena` موجودة ومُختبرة لكنها غير
> مستخدمة في مسار الخادم؛ معاملات المسارات تُفكّ وتُنسخ إلى `String`، والجسم يُنسخ إلى `Bytes`
> قبل تنفيذ المعالج. أُضيف لاحقاً دعم WebSocket كامل (handshake + connection driver +
> `app.ws`)، ومحلل `multipart/form-data`، ودعم `Range` للملفات الثابتة. راجع
> [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).

### المرجع الكامل لكافة عناصر، مكونات، وصناديق إطار العمل مع أمثلة عملية شاملة

---

## 📑 جدول المحتويات (Table of Contents)
1. [نظرة عامة على بنية الإطار وهندسته (System Architecture)](#1-نظرة-عامة-على-بنية-الإطار-وهندسته)
2. [التثبيت والإعداد في المشاريع (Installation & Cargo Setup)](#2-التثبيت-والإعداد-في-المشاريع)
3. [عنصر التطبيق الأساسي (AetherApp & Server Configuration)](#3-عنصر-التطبيق-الأساسي)
4. [محرك المسارات والمعالجات (Routing: Sync & Async Handlers)](#4-محرك-المسارات-والمعالجات)
5. [سياق الطلب واستخراج البيانات (RequestContext & Extractors)](#5-سياق-الطلب-واستخراج-البيانات)
6. [نظام الاستجابات والتحويل التلقائي (Response & IntoResponse)](#6-نظام-الاستجابات-والتحويل-التلقائي)
7. [إدارة الحالة المشتركة للتطبيق (Global Application State)](#7-إدارة-الحالة-المشتركة-للتطبيق)
8. [البرمجيات الوسيطة والأمان (Middleware & Web Security)](#8-البرمجيات-الوسيطة-والأمان)
9. [محرك الذاكرة فائق السرعة (RequestArena & Zero-Copy Engine)](#9-محرك-الذاكرة-فائق-السرعة)
10. [التحقق السريع بـ SIMD والتسلسل (Validation & Fast JSON Serialization)](#10-التحقق-السريع-بـ-simd-والتسلسل)
11. [حاوية حقن التبعيات والرسم البياني (Dependency Injection & DAG)](#11-حاوية-حقن-التبعيات-والرسم-البياني)
12. [بروتوكول WebSocket فائق السرعة (RFC 6455 Frame Parser)](#12-بروتوكول-websocket-فائق-السرعة)
13. [المولد التلقائي لمواصفات OpenAPI v3.1](#13-المولد-التلقائي-لمواصفات-openapi-v31)
14. [محرك المراقبة والتشخيص الذكي (Observability & Terminal Diagnostics)](#14-محرك-المراقبة-والتشخيص-الذكي)
15. [خدمة الملفات الثابتة و ETag (Static Files & Asset Serving)](#15-خدمة-الملفات-الثابتة-و-etag)
16. [مشروع إنتاجي متكامل شامل لجميع العناصر (Full Production Blueprint)](#16-مشروع-إنتاجي-متكامل-شامل-لجميع-العناصر)

---

## 1. نظرة عامة على بنية الإطار وهندسته

تم تصميم **Aether** وفق مبادئ **Zero-Cost Abstractions** و **Zero-Copy Parsing** و **Cache-Line Alignment**، حيث تم تقسيمه إلى حزم معيارية معزولة داخل Cargo Workspace:

| الحزمة (Crate) | المسؤولية الفنية والوظيفية |
| :--- | :--- |
| `aether` | واجهة الاستخدام الموحدة للواجهة والمطور (Facade & Prelude) |
| `aether-core` | مخصص الذاكرة السريعة `RequestArena` ومحرك التشخيص `Diagnostic Engine` |
| `aether-http` | محلل طلبات HTTP/1.1 و HTTP/2 بدون حجز ذاكرة، الردود، والملفات الثابتة |
| `aether-router` | شجرة المسارات `RadixRouter` مع فحص الأفعال بالـ 64-Bit Bitmask |
| `aether-runtime` | حلقة أحداث العامل (Worker Event Loop) ومعالجة اتصالات TCP |
| `aether-middleware` | الترويسات الأمنية، CORS، ومسجل الطلبات الدقيق بالميكروثانية |
| `aether-security` | التشفير السريع، التحقق من JWT، والمقارنة بزمن ثابت `constant_time_eq` |
| `aether-validation` | التحقق من صحة البريد الإلكتروني والنصوص بسرعة تعليمات SIMD |
| `aether-serialization`| تسلسل وإلغاء تسلسل JSON الفائق عبر `sonic-rs` و `serde` |
| `aether-dependency` | حاوية حقن التبعيات بحل شجرة الرسم البياني `Topological DAG Sort` |
| `aether-websocket` | محلل إطارات الـ WebSocket (RFC 6455) بدون تخصيص ذاكرة |
| `aether-openapi` | مولد وثائق ومخططات OpenAPI v3.1 JSON تلقائياً |
| `aether-observability`| مؤقتات الأداء الميكروية `RequestTimer` |

---

## 2. التثبيت والإعداد في المشاريع

للبدء في أي مشروع Rust جديد، أنشئ مجلد المشروع وأضف الاعتماديات التالية إلى `Cargo.toml`:

```toml
[package]
name = "my_aether_api"
version = "0.1.0"
edition = "2024"

[dependencies]
aether = { path = "../framework/aether" } # أو استخدام الإصدار في حال النشر
tokio = { version = "1.38", features = ["full"] }
bytes = "1.6"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

---

## 3. عنصر التطبيق الأساسي (`AetherApp`)

`AetherApp` هو الهيكل الأساسي الذي يقوم بتجميع المسارات، الحالة، وسيطات الأمان، وتكوين خادم الشبكة.

```rust
use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. إنشاء نسخة من التطبيق
    let mut app = AetherApp::new();

    // 2. تخصيص حدود الخادم (Server Limits) - اختياري
    let limits = ServerLimits {
        max_headers: 64,
        max_header_bytes: 8192,
        max_body_bytes: 10 * 1024 * 1024, // 10MB
    };

    // 3. تشغيل الخادم
    app.listen("0.0.0.0:3000").await
}
```

---

## 4. محرك المسارات والمعالجات (Routing Engine)

### أ) المعالجات المتزامنة السريعة (Synchronous Handlers):
للعمليات الحسابية الخفيفة التي لا تتطلب I/O (تعمل في أقل من 20 نانوثانية):
```rust
app.get("/ping", |_req| "pong");

app.post("/calculate", |req| {
    format!("Payload byte length: {}", req.body.len())
});
```

### ب) المعالجات غير المتزامنة (Asynchronous Handlers):
لقواعد البيانات والاتصالات الشبكية:
```rust
app.get_async("/users", |req: RequestContext| async move {
    // محاكاة عملية I/O لقاعدة بيانات
    tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
    Response::json(bytes::Bytes::from(r#"[{"id":1,"name":"حمزة"}]"#))
});

app.post_async("/orders/create", |req: RequestContext| async move {
    // قراءة البيانات ومعالجتها عبر async/await
    Response::ok().header("X-Order-Status", "Created")
});
```

### ج) استخراج وتحويل متغيرات المسار (Dynamic Route Parameters & Typed Parsing):
تُستخرج المتغيرات بتقنية Zero-Copy بدون أي حجز للذاكرة، مع إمكانية التحويل التلقائي للأنواع (`i32`, `u64`, `bool`, إلخ):
```rust
// 1. استخراج كنص خام
app.get("/users/{id}", |req| {
    let user_id = req.param("id").unwrap_or("0");
    format!(r#"{{"user_id":"{}"}}"#, user_id)
});

// 2. استخراج وتحويل تلقائي مباشر إلى أرقام (Typed Extractor)
app.get("/products/{id}", |req| {
    let product_id: u64 = req.param_parse("id").unwrap_or(0);
    format!("Product ID: {}", product_id)
});

app.get("/orgs/{org_id}/repos/{repo_name}", |req| {
    let org = req.param("org_id").unwrap_or("unknown");
    let repo = req.param("repo_name").unwrap_or("unknown");
    format!("Organization: {}, Repository: {}", org, repo)
});
```

### د) تجميع المسارات مع دعم كامل لجميع أفعال REST (`Route Grouping`):
يدعم `RouteGroup` كافة أفعال HTTP (`get`, `post`, `put`, `delete`, `patch`) المتزامنة وغير المتزامنة:
```rust
app.group("/api/v1", |v1| {
    v1.get("/status", |_req| r#"{"status":"v1_active"}"#);
    v1.get_async("/metrics", |_req: RequestContext| async move { r#"{"cpu_load":"4%"}"# });
    v1.post_async("/auth/login", |req: RequestContext| async move { /* معالجة الدخول */ Response::ok() });
    v1.put_async("/users/{id}", |req: RequestContext| async move { /* تحديث مستخدم */ Response::ok() });
    v1.delete_async("/items/{id}", |req: RequestContext| async move { /* حذف عنصر */ Response::ok() });
});

app.group("/admin", |admin| {
    admin.get("/panel", |_req| Response::html("<h1>Admin Panel</h1>"));
});
```

---

## 5. سياق الطلب واستخراج البيانات (`RequestContext`)

يمثل `RequestContext` الكائن الشامل لكل تفاصيل الطلب الوارد مع دوال مساعدة فائقة الراحة:

```rust
app.post("/api/checkout", |req| {
    // 1. المتغيرات في مسار الرابط (نص أو محولة رقمياً)
    let user_id = req.param("id");
    let user_id_num: i32 = req.param_parse("id").unwrap_or(1);

    // 2. معرف الطلب الفريد للتتبع (Distributed Tracing ID)
    let trace_id = req.request_id; // مثال: "req_000000000001"

    // 3. الترويسات والكوكيز (Headers & Cookies)
    let auth_header = req.header("authorization");
    let session_cookie = req.cookie("session_id");

    // 4. متغيرات الاستعلام (Query String) - نص أو محولة رقمياً
    let page: i32 = req.query_parse("page").unwrap_or(1);
    let filter = req.query("filter").unwrap_or("all");

    // 5. قراءة نماذج الويب المرسلة عبر Form POST (URL-Encoded Form Data)
    let form_fields = req.form_data(); // يرجع HashMap<String, String> مفككة الرموز %XX و + تلقائياً
    let action = form_fields.get("action");

    // 6. قراءة الـ Body كنص UTF-8 أو كبايتات خام
    let body_text = req.body_str().unwrap_or("");
    let raw_bytes = req.body;

    // 7. تحويل الـ JSON تلقائياً إلى بنية Struct
    #[derive(serde::Deserialize)]
    struct OrderRequest {
        product_id: u64,
        quantity: u32,
    }

    if let Ok(order) = req.json::<OrderRequest>() {
        println!("Received order for product: {}", order.product_id);
    }

    Response::ok()
});
```

---

## 6. نظام الاستجابات والتحويل التلقائي (`Response & IntoResponse`)

### أ) بناء الاستجابات عبر `Response`:
```rust
// 1. استجابة نجاح 200 OK فارغة
Response::ok();

// 2. استجابة HTML من نص ثابت (Zero-Copy &'static str)
Response::html("<h1>صفحة ترحيبية ثابتة</h1>");

// 3. استجابة HTML من نص ديناميكي أو قوالب (String / Tera / Askama)
Response::html_dynamic(format!("<h1>مرحباً بك يا {}!</h1>", username));

// 4. قراءة وخدمة ملف مباشرة من القرص الصلب مع اكتشاف نوع الـ MIME تلقائياً
Response::file("public/index.html"); // يرجع Result<Response, std::io::Error>

// 5. استجابة JSON مع حالة 200 الافتراضية
Response::json(bytes::Bytes::from(r#"{"status":"success"}"#));

// 6. استجابة JSON مع رمز حالة مخصص (Custom Status Code)
Response::json_status(401, "Unauthorized", r#"{"error":"Invalid Credentials"}"#);
Response::json_status(400, "Bad Request", r#"{"error":"Missing fields"}"#);

// 7. استجابة بايتات من أي مصدر مع تحديد نوع المحتوى
Response::bytes(image_bytes, "image/png");

// 8. إعادة التوجيه (Redirect)
Response::redirect("/dashboard", 302);

// 9. إضافة ترويسات وكوكيز برمجية بنمط الـ Builder
let res = Response::ok()
    .header("X-Custom-Engine", "Aether")
    .set_cookie("session_id", "token_12345", true, true, "Strict", Some(86400));
```

### ب) التحويل التلقائي (`IntoResponse`):
```rust
// نص مباشر
app.get("/text", |_req| "نص عادي بترميز UTF-8");

// كود حالة مع نص
app.get("/forbidden", |_req| (403, "غير مصرح بالدخول"));

// معالجة الأخطاء عبر Result
app.get("/check", |_req| -> Result<&'static str, (u16, &'static str)> {
    let authorized = true;
    if authorized {
        Ok("تم التحقق بنجاح")
    } else {
        Err((401, "مطلوب تسجيل الدخول"))
    }
});
```

---

## 7. إدارة الحالة المشتركة للتطبيق (`Global Application State`)

لمشاركة اتصالات قواعد البيانات، المخابئ، وحالة الخادم بين كافة المسارات:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

// 1. تعريف بنية الحالة
struct AppState {
    db_pool: String,
    total_requests: AtomicUsize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 2. تسجيل الحالة داخل التطبيق
    app.manage(AppState {
        db_pool: "PostgresConnectionPool".to_string(),
        total_requests: AtomicUsize::new(0),
    });

    // 3. استخراج الحالة واستخدامها داخل أي معالج
    app.get("/stats", |req| {
        if let Some(state) = req.state::<AppState>() {
            let count = state.total_requests.fetch_add(1, Ordering::Relaxed) + 1;
            format!("Database: {}, Total Served Requests: {}", state.db_pool, count)
        } else {
            "AppState is not configured".into()
        }
    });

    app.listen("0.0.0.0:3000").await
}
```

---

## 8. البرمجيات الوسيطة والأمان (`Middleware & Security`)

### أ) تفعيل ترويسات الأمان المتقدمة تلقائياً:
```rust
// يفعل تلقائياً HSTS, X-Content-Type-Options: nosniff, X-Frame-Options: DENY, X-XSS-Protection
app.enable_security_headers();
```

### ب) تفعيل سياسة تبادل الموارد (CORS):
```rust
app.enable_cors(CorsMiddleware::new(CorsConfig {
    allow_origin: "*",
    allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
    allow_headers: "Content-Type, Authorization, X-Request-ID",
    max_age_secs: 86400,
}));
```

### ج) المقارنة في زمن ثابت لمنع هجمات التوقيت (`constant_time_eq`):
```rust
use aether_security::constant_time_eq;

let expected_hash = b"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
let provided_hash = b"9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

let is_valid = constant_time_eq(expected_hash, provided_hash);
```

### د) التحقق من توقيع JWT:
```rust
use aether_security::JwtValidator;

let validator = JwtValidator::new(b"your-256-bit-secret-key-here");
let raw_token = "header.payload.signature";

if let Ok((header, payload)) = validator.verify_signature(raw_token) {
    println!("Token Validated! Header: {}, Payload: {}", header, payload);
}
```

---

## 9. محرك الذاكرة فائق السرعة (`RequestArena`)

يوفر Aether مخصص ذاكرة Bump Allocator مخصص لكل اتصال TCP:
- لا يقوم باستدعاء `malloc` أو `free` في كل طلب HTTP.
- تتم إعادة التصفير عبر تعديل مؤشر واحد $O(1)$ (`arena.reset()`).

```rust
use aether_core::arena::RequestArena;

// إنشاء Arena بسعة 4096 بايت (محاذاة 64-byte للـ Cache Line)
let arena = RequestArena::with_capacity(4096);

// حجز مساحة لشريحة بايتات دون حجز على الـ Heap
let slice = arena.alloc_slice(b"Zero-Copy payload data").unwrap();

// تصفير فوري في أقل من 1 نانوثانية للطلب التالي
arena.reset();
```

---

## 10. التحقق السريع بـ SIMD والتسلسل

### أ) التحقق فائق السرعة عبر تعليمات المعالج (SIMD):
```rust
use aether_validation::{validate_email_format, validate_ascii_fast};

let is_email = validate_email_format("user@aether-framework.io"); // true
let is_ascii = validate_ascii_fast(b"High Performance Rust");     // true
```

### ب) التسلسل وإلغاء التسلسل الفائق لـ JSON:
```rust
use aether_serialization::json::{to_bytes, from_slice};
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Product {
    id: u64,
    title: String,
    price: f64,
}

let item = Product { id: 1, title: "Aether Cloud Node".into(), price: 49.99 };

// تحويل إلى بايتات بسرعة قصوى
let json_bytes = to_bytes(&item).unwrap();

// استرجاع الكائن من البايتات
let parsed: Product = from_slice(&json_bytes).unwrap();
```

---

## 11. حاوية حقن التبعيات والرسم البياني (`Dependency Injection`)

بناء رسم بياني موجه غير دوري (DAG) للخدمات مع التحقق من عدم وجود دورات مفرغة وقت الإقلاع:

```rust
use aether_dependency::{DependencyContainer, DependencyScope};

let mut container = DependencyContainer::new();

// تسجيل الخدمات
let config_node = container.register("ConfigService", DependencyScope::Singleton);
let db_node = container.register("DatabasePool", DependencyScope::Singleton);
let auth_node = container.register("AuthService", DependencyScope::Scoped);

// ربط شجرة الاعتماديات
container.add_dependency(auth_node, db_node).unwrap();
container.add_dependency(db_node, config_node).unwrap();

// التحقق وترتيب التسلسل تلقائياً (Topological Sorting)
let execution_order = container.resolve_build_order().unwrap();
```

---

## 12. بروتوكول WebSocket (RFC 6455)

الخادم يدعم WebSocket كاملاً: يتحقق من الـ handshake، ويكتب `101 Switching Protocols`، ثم
يشغّل المعالج مع connection driver يجمّع الرسائل المجزأة ويرد على `Ping` ويغلق الاتصال
بشكل صحيح. الرسائل الافتراضية محدودة بـ 16 MiB للإطار والرسالة:

```rust
use aether::prelude::*;

app.ws("/ws", |_req, mut socket| async move {
    while let Ok(Some(message)) = socket.recv().await {
        match message {
            WsMessage::Text(text) => {
                let _ = socket.send_text(&text).await;
            }
            WsMessage::Binary(bytes) => {
                let _ = socket.send_binary(&bytes).await;
            }
        }
    }
});
```

طلبات الـ handshake غير الصحيحة تُرفض قبل تنفيذ المعالج: `400` للمفتاح/الترويسات غير
الصالحة، و`426` مع `Sec-WebSocket-Version: 13` للإصدار غير المدعوم، و`405` لغير `GET`.
التعطيل compression وsubprotocol negotiation غير مدعومين بعد.

يمكن أيضاً استخدام محلل الإطارات المنخفض المستوى بدون أي حجز للذاكرة:

```rust
use aether_websocket::{parse_frame, WsOpcode};

let raw_ws_frame = [0x81, 0x05, 0x48, 0x65, 0x6C, 0x6C, 0x6F]; // Text Frame: "Hello"

if let Ok(Some((frame, consumed))) = parse_frame(&raw_ws_frame) {
    if frame.opcode == WsOpcode::Text {
        let text = std::str::from_utf8(frame.payload).unwrap();
        println!("WebSocket message received: {}", text);
    }
}
```

---

## 13. المولد التلقائي لمواصفات OpenAPI v3.1

إنشاء مخطط توثيق Swagger / OpenAPI 3.1 بدون التأثير على مسار معالجة الطلبات:

```rust
use aether_openapi::{OpenApiSpec, PathItem, Operation, ResponseSpec};

let mut spec = OpenApiSpec::new("Aether High-Performance REST API", "1.0.0");

let mut get_users = Operation::default();
get_users.summary = "Retrieve all registered users".to_string();
get_users.responses.insert("200".to_string(), ResponseSpec {
    description: "Successful response returning array of users".to_string(),
});

let mut item = PathItem::default();
item.get = Some(get_users);
spec.paths.insert("/api/v1/users".to_string(), item);

let openapi_json = spec.to_json().unwrap();
```

---

## 14. محرك المراقبة والتشخيص الذكي (Observability & Diagnostics)

### أ) حساب أزمنة الطلبات بالميكروثانية:
```rust
use aether_observability::RequestTimer;

let timer = RequestTimer::start();
// معالجة الطلب...
let elapsed_micros = timer.elapsed_micros();
```

### ب) محرك التشخيص الملون للطرفية (Rich Terminal Diagnostics):
```rust
use aether_core::diagnostic::{Diagnostic, ErrorCode};

let diag = Diagnostic::new(ErrorCode::E1001ServerStartupFailed, "TCP Bind failed: Address already in use")
    .with_context("Port", "3000")
    .with_hint("Port 3000 is occupied by another process.")
    .with_suggestion("Set AETHER_PORT=3001 to run on a different port.")
    .with_suggestion("Kill the conflicting process: lsof -ti:3000 | xargs kill -9");

eprintln!("{}", diag.format_terminal(true));
```

---

## 15. خدمة الملفات الثابتة والأصول (Static Files & Asset Serving)

يوفر Aether طريقتين مريحتين للغاية لخدمة الملفات الثابتة (CSS, JS, صور, خطوط):

### أ) خدمة أصل برمجي من الذاكرة مباشرة (`app.static_asset`):
الأسرع على الإطلاق لتضمين ملفات الـ CSS والـ JS داخل التطبيق (Zero-Disk-I/O):
```rust
let learning_css = include_bytes!("../static/css/learning.css");
app.static_asset("/static/css/learning.css", "text/css; charset=utf-8", learning_css);
```

### ب) خدمة ملف مباشرة من القرص الصلب (`app.static_file`):
```rust
app.static_file("/favicon.ico", "public/favicon.ico");
app.static_file("/robots.txt", "public/robots.txt");
```

---

## 16. دمج محركات القوالب (Tera / Askama Template Engines)

يمكن دمج أي محرك قوالب HTML بسهولة تامة مع حالة التطبيق المشتركة `app.manage`:

```rust
use tera::{Tera, Context};

// 1. تعريف حالة التطبيق متضمنة Tera
struct AppState {
    tera: Tera,
}

// 2. دالة مساعدة لعرض قوالب Tera في استجابة Aether
fn render_template(tera: &Tera, template_name: &str, ctx: &Context) -> Response {
    match tera.render(template_name, ctx) {
        Ok(html_string) => Response::html_dynamic(html_string),
        Err(err) => {
            Response::json_status(500, "Internal Server Error", format!(r#"{{"error":"Template render error: {}"}}"#, err))
        }
    }
}

// 3. الاستخدام داخل معالجات المسارات
app.get_async("/dashboard", |req: RequestContext| async move {
    let state = req.state::<AppState>().expect("State missing");
    let mut ctx = Context::new();
    ctx.insert("page_title", "لوحة التحكم");
    ctx.insert("user_id", &101);
    render_template(&state.tera, "dashboard.html", &ctx)
});
```

---

## 17. مشروع إنتاجي متكامل شامل لجميع العناصر (Full Production Blueprint)

إليك كود خادم ويب إنتاجي متكامل جاهز للتشغيل يوضح دمج جميع الميزات في ملف واحد:

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{sleep, Duration};

// 1. حالة التطبيق المشتركة
struct ProductionState {
    service_name: &'static str,
    active_requests: AtomicU64,
}

// 2. نماذج البيانات (DTOs)
#[derive(Deserialize)]
struct CreateAccountDto {
    username: String,
    email: String,
}

#[derive(Serialize)]
struct AccountResponseDto {
    id: u64,
    username: String,
    email: String,
    created_at_epoch_ms: u128,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // تسجيل الحالة المشتركة
    app.manage(ProductionState {
        service_name: "Aether Enterprise Microservice",
        active_requests: AtomicU64::new(0),
    });

    // تفعيل البرمجيات الوسيطة والأمان
    app.enable_security_headers();
    app.enable_cors(CorsMiddleware::new(CorsConfig {
        allow_origin: "*",
        allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
        allow_headers: "Content-Type, Authorization, X-Request-ID",
        max_age_secs: 86400,
    }));

    // الصفحة الرئيسية HTML
    app.get("/", |_req| {
        Response::html(concat!(
            "<!DOCTYPE html><html lang='ar' dir='rtl'><head><meta charset='utf-8'>",
            "<title>⚡ Aether Enterprise Core</title>",
            "<style>body{background:#0b0f19;color:#fff;font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}.c{text-align:center;padding:40px;background:#111827;border-radius:16px;border:1px solid #1f2937}h1{color:#38bdf8}a{display:inline-block;margin:8px;padding:8px 16px;background:#2563eb;color:#fff;text-decoration:none;border-radius:6px}</style></head>",
            "<body><div class='c'><h1>⚡ خادم Aether الفائق يعمل الآن!</h1><p>استجابة بأقل من 1 ميكروثانية</p><a href='/api/v1/health'>Health Check</a><a href='/api/v1/users/101'>User 101</a></div></body></html>"
        ))
    });

    // تجميع مسارات API v1
    app.group("/api/v1", |v1| {
        // فحص الصحة مع قراءة الحالة
        v1.get("/health", |req| {
            let state = req.state::<ProductionState>().unwrap();
            let count = state.active_requests.fetch_add(1, Ordering::Relaxed) + 1;
            format!(
                r#"{{"status":"healthy","service":"{}","served_count":{}}}"#,
                state.service_name, count
            )
        });

        // إنشاء مستخدم جديد عبر POST مع قراءة الـ JSON
        v1.post_async("/accounts", |req: RequestContext| async move {
            let payload: CreateAccountDto = match req.json() {
                Ok(dto) => dto,
                Err(_) => return Response::status(400, "Invalid JSON body"),
            };

            // محاكاة معالجة غير متزامنة
            sleep(Duration::from_millis(2)).await;

            let response_dto = AccountResponseDto {
                id: 99401,
                username: payload.username,
                email: payload.email,
                created_at_epoch_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis(),
            };

            let out_bytes = serde_json::to_string(&response_dto).unwrap();
            Response::json(bytes::Bytes::from(out_bytes))
        });

        // جلب حساب محدد بالـ Dynamic Parameter
        v1.get_async("/users/{id}", |req: RequestContext| async move {
            let user_id = req.param("id").unwrap_or("0");
            sleep(Duration::from_millis(1)).await;
            format!(r#"{{"user_id":"{}","status":"active"}}"#, user_id)
        });
    });

    // تشغيل الخادم على العنوان والمنفذ المحددين
    app.listen("0.0.0.0:3000").await
}
```

---

تم إنشاء هذا التوثيق ليكون دليلاً مرجعياً دائماً داخل مجلد التوثيق في المشروع (`framework/docs/FULL_FRAMEWORK_DOCUMENTATION_AR.md`).
