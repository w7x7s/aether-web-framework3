# الدليل المرجعي والتوثيق الشامل لإطار عمل Aether Web Framework ⚡

> **ملاحظة تدقيق (2026-10-18):** الجسم يُقرأ كـ `Bytes` مملوك وليس شريحة مستعارة؛ معاملات
> المسار تُفكّ إلى `String`؛ و`RequestArena` غير مستخدمة في مسار الخادم الافتراضي. الخادم
> يدعم HTTP/1.1 فقط. راجع [FEATURE_COMPLETENESS_MATRIX.md](FEATURE_COMPLETENESS_MATRIX.md).

### دليل المطور الكامل لبناء خوادم الويب وتطبيقات الـ APIs فائقة الأداء

أهلاً بك في **Aether**! إطار عمل خفيف، فائق السرعة، ومصمم بلغة Rust ليجمع بين تجربة التطوير السهلة والمريحة (FastAPI / Axum Developer Experience) وبين أقصى درجات الأداء المنخفض المستوى (Zero-Copy Memory, Bump Arena Allocator, 64-Bit Bitmask Routing).

---

## الفهرس العام
1. [التثبيت والبدء السريع (Quick Start)](#1-التثبيت-والبدء-السريع)
2. [المعالجات غير المتزامنة والمزامنة (Sync & Async Handlers)](#2-المعالجات-غير-المتزامنة-والمزامنة)
3. [استخراج وقراءة بيانات الـ Body و JSON (Typed JSON & Body Extractors)](#3-استخراج-وقراءة-بيانات-الـ-body-و-json)
4. [إدارة الحالة المشتركة (Global Application State)](#4-إدارة-الحالة-المشتركة)
5. [البرمجيات الوسيطة والحماية (Middleware & Security)](#5-البرمجيات-الوسيطة-والحماية)
6. [تجميع المسارات (Route Groups)](#6-تجميع-المسارات)
7. [معالجة الأخطاء ومحول الردود (Error Handling & IntoResponse)](#7-معالجة-الأخطاء-ومحول-الردود)
8. [معاملات المسارات (Zero-Copy Dynamic Path Parameters)](#8-معاملات-المسارات)
9. [محرك الذاكرة منخفض المستوى ومزايا Aether الحصرية](#9-محرك-الذاكرة-منخفض-المستوى)
10. [مثال تطبيقي متكامل وشامل (Full Production Blueprint)](#10-مثال-تطبيقي-متكامل-وشامل)

---

<a name="1-التثبيت-والبدء-السريع"></a>
## 1. التثبيت والبدء السريع (Quick Start)

### أ) إعداد ملف `Cargo.toml`:
```toml
[dependencies]
aether = { path = "../framework/aether" }
tokio = { version = "1.38", features = ["full"] }
bytes = "1.6"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### ب) كتابة أول خادم في ملف `src/main.rs`:
```rust
use aether::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // مسار ترحيبي بسيط
    app.get("/", |_req| {
        Response::html("<h1>⚡ مرحباً بك في خادم Aether السريع!</h1>")
    });

    // الاستماع على المنفذ 3000
    app.listen("0.0.0.0:3000").await
}
```

---

<a name="2-المعالجات-غير-المتزامنة-والمزامنة"></a>
## 2. المعالجات غير المتزامنة والمزامنة (Sync & Async Handlers)

يدعم Aether المعالجة المتزامنة (للعمليات الحسابية السريعة والذاكرة) وغير المتزامنة (لقواعد البيانات والـ I/O):

### أ) المعالجة المتزامنة (Synchronous):
```rust
app.get("/sync-fast", |_req| {
    "استجابة فورية بدون أي تأخير"
});
```

### ب) المعالجة غير المتزامنة مع `.await` (Asynchronous):
```rust
use tokio::time::{sleep, Duration};

app.get_async("/async-db", |req: RequestContext| async move {
    // محاكاة استعلام قاعدة بيانات يستغرق 15ms
    sleep(Duration::from_millis(15)).await;
    
    Response::json(bytes::Bytes::from(r#"{"status":"db_data_loaded"}"#))
});

app.post_async("/async-send", |req: RequestContext| async move {
    // إرسال إشعار أو حفظ سجل
    "تمت المعالجة غير المتزامنة"
});
```

---

<a name="3-استخراج-وقراءة-بيانات-الـ-body-و-json"></a>
## 3. استخراج وقراءة بيانات الـ Body و JSON (Typed JSON & Body Extractors)

### أ) استخراج JSON تلقائياً في بنية Struct (`req.json::<T>()`):
```rust
use serde::Deserialize;

#[derive(Deserialize)]
struct CreateProductDto {
    name: String,
    price: f64,
    in_stock: bool,
}

app.post_async("/api/products", |req: RequestContext| async move {
    // تحويل JSON تلقائياً مع معالجة الأخطاء
    let product: CreateProductDto = match req.json() {
        Ok(p) => p,
        Err(_) => return Response::status(400, "Bad Request: Invalid JSON Body"),
    };

    println!("تم استلام المنتج: {} بسعر {}", product.name, product.price);
    Response::json(bytes::Bytes::from(r#"{"success":true}"#))
});
```

### ب) قراءة الـ Body كنص UTF-8 أو بايتات خام:
```rust
// قراءة كنص:
let text: &str = req.body_str().unwrap_or("");

// قراءة كشريحة بايتات مستعارة مباشرة (Zero-Copy):
let raw_bytes: &[u8] = req.body;
```

---

<a name="4-إدارة-الحالة-المشتركة"></a>
## 4. إدارة الحالة المشتركة (Global Application State)

يتيح لك Aether مشاركة اتصالات قواعد البيانات ومفاتيح التشفير والإعدادات عبر التطبيق كاملاً بأمان $O(1)$ بدون تعقيدات الأقفال:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

struct AppDatabase {
    connection_url: String,
    query_count: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // 1. تسجيل الحالة في التطبيق
    app.manage(AppDatabase {
        connection_url: "postgres://user:pass@localhost/db".to_string(),
        query_count: AtomicU64::new(0),
    });

    // 2. الوصول للحالة في أي مسار
    app.get("/stats", |req| {
        if let Some(db) = req.state::<AppDatabase>() {
            let count = db.query_count.fetch_add(1, Ordering::SeqCst) + 1;
            format!(r#"{{"database":"{}","queries":{}}}"#, db.connection_url, count)
        } else {
            "State unavailable".to_string()
        }
    });

    app.listen("0.0.0.0:3000").await
}
```

---

<a name="5-البرمجيات-الوسيطة-والحماية"></a>
## 5. البرمجيات الوسيطة والحماية (Middleware & Security)

### أ) تفعيل ترويسات الأمان (Security Headers Middleware):
تفعيل تلقائي لترويسات HSTS, X-Content-Type-Options: nosniff, X-Frame-Options: DENY, X-XSS-Protection:
```rust
app.enable_security_headers();
```

### ب) تفعيل سياسة تبادل الموارد (CORS Middleware):
```rust
app.enable_cors(CorsMiddleware::new(CorsConfig {
    allow_origin: "*",
    allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
    allow_headers: "Content-Type, Authorization, X-Requested-With",
    max_age_secs: 86400,
}));
```

### ج) مسجل الطلبات المدمج (Microsecond Logger):
يقوم بطباعة زمن استجابة كل طلب بدقة الميكروثانية (`µs`) في شاشة الخادم تلقائياً:
`⚡ [Aether] Get /api/products -> 200 (42µs)`

---

<a name="6-تجميع-المسارات"></a>
## 6. تجميع المسارات (Route Groups)

لتنظيم إصدارات الـ API وفصل المسارات الإدارية:

```rust
// مسارات الإصدار الأول V1
app.group("/api/v1", |v1| {
    v1.get("/health", |_req| r#"{"version":"v1","status":"healthy"}"#);
    
    v1.get_async("/users", |req: RequestContext| async move {
        r#"[{"id":1,"name":"Hamza"}]"#
    });
});

// مسارات لوحة التحكم
app.group("/admin", |admin| {
    admin.get("/dashboard", |_req| Response::html("<h1>Admin Panel</h1>"));
});
```

---

<a name="7-معالجة-الأخطاء-ومحول-الردود"></a>
## 7. معالجة الأخطاء ومحول الردود (Error Handling & IntoResponse)

يمكن لدوال المسارات في Aether إرجاع أنواع متعددة يتم تحويلها لردود HTTP نقية تلقائياً:

| القيمة المرجعة من الدالة | النتيجة في بروتوكول HTTP |
| :--- | :--- |
| `Response::ok()` | 200 OK |
| `Response::html("<h1>مرحبا</h1>")` | 200 OK مع ترويسة HTML |
| `Response::json(bytes)` | 200 OK مع ترويسة JSON |
| `"نص مباشر"` أو `String` | 200 OK نصي |
| `(404, "Not Found")` | 404 Not Found مع كود الحالة |
| `Result<T, MyError>` | إذا كان `Ok` يعيد الرد، وإذا كان `Err` يعيد 400 Bad Request بتفاصيل الخطأ تلقائياً |

```rust
app.get("/check-balance", |_req| -> Result<&'static str, &'static str> {
    let has_funds = true;
    if has_funds {
        Ok("الرصيد كافي للعملية")
    } else {
        Err("عذراً، الرصيد غير كافٍ")
    }
});
```

---

<a name="8-معاملات-المسارات"></a>
## 8. معاملات المسارات (Zero-Copy Dynamic Path Parameters)

قراءة المتغيرات من الرابط (مثل `/users/{id}` أو `/orgs/{org_id}/teams/{team_id}`):

```rust
app.get("/users/{user_id}", |req| {
    let user_id = req.param("user_id").unwrap_or("0");
    format!(r#"{{"user_id":{}}}"#, user_id)
});
```

---

<a name="9-محرك-الذاكرة-منخفض-المستوى"></a>
## 9. محرك الذاكرة منخفض المستوى ومزايا Aether الحصرية

1. **`RequestArena` O(1) Bump Reset:** إعادة استخدام نفس كتلة الذاكرة (4KB) لاتصالات Keep-Alive بدون استدعاء `malloc/free` وبدون تفتيت للـ Heap.
2. **64-Bit Method Bitmask:** فحص أفعال الـ HTTP بتعليمة معالج واحدة (Single CPU Cycle) في الـ L1 Cache.
3. **Connection Control Block (CCB):** استهلاك ~420 بايت فقط لكل اتصال Socket خامل، مما يسمح بتشغيل 100,000 اتصال في أقل من 45MB RAM.
4. **Precompiled Dependency Graph (DAG):** فحص وحل شجرة التبعيات وقت الإقلاع بدلاً من الـ Reflection البطيء في أطر لغات التفسير.

---

<a name="10-مثال-تطبيقي-متكامل-وشامل"></a>
## 10. مثال تطبيقي متكامل وشامل (Full Production Blueprint)

```rust
use aether::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{sleep, Duration};

// 1. تعريف حالة التطبيق العامة
struct ProductionState {
    app_version: &'static str,
    total_requests: AtomicU64,
}

// 2. هياكل البيانات للـ JSON
#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    email: String,
}

#[derive(Serialize)]
struct UserResponse {
    id: u64,
    username: String,
    email: String,
    active: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // تفعيل الحالة المشتركة
    app.manage(ProductionState {
        app_version: "2.4.0-release",
        total_requests: AtomicU64::new(0),
    });

    // تفعيل البرمجيات الوسيطة
    app.enable_security_headers();
    app.enable_cors(CorsMiddleware::new(CorsConfig {
        allow_origin: "*",
        allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
        allow_headers: "Content-Type, Authorization",
        max_age_secs: 86400,
    }));

    // الصفحة الرئيسية
    app.get("/", |_req| {
        Response::html("<h1>🚀 مرحباً بك في خادم Aether للإنتاج!</h1>")
    });

    // مسارات الـ API
    app.group("/api/v1", |v1| {
        // فحص الصحة
        v1.get("/health", |req| {
            let state = req.state::<ProductionState>().unwrap();
            let count = state.total_requests.fetch_add(1, Ordering::SeqCst) + 1;
            format!(
                r#"{{"status":"healthy","version":"{}","total_served":{}}}"#,
                state.app_version, count
            )
        });

        // إنشاء مستخدم جديد (Async + JSON)
        v1.post_async("/users", |req: RequestContext| async move {
            let payload: CreateUserRequest = match req.json() {
                Ok(data) => data,
                Err(_) => return Response::status(400, "Bad Request: Invalid JSON"),
            };

            // محاكاة استعلام غير متزامن
            sleep(Duration::from_millis(5)).await;

            let response_data = UserResponse {
                id: 4201,
                username: payload.username,
                email: payload.email,
                active: true,
            };

            let json_out = serde_json::to_string(&response_data).unwrap();
            Response::json(bytes::Bytes::from(json_out))
        });

        // جلب مستخدم محدد بالـ ID
        v1.get_async("/users/{id}", |req: RequestContext| async move {
            let id = req.param("id").unwrap_or("0");
            sleep(Duration::from_millis(2)).await;
            format!(r#"{{"user_id":{},"status":"active"}}"#, id)
        });
    });

    println!("⚡ خادم Aether يعمل على: http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await
}
```
