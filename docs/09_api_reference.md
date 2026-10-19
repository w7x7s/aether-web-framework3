# 09. المرجع الشامل لدوال وكائنات Aether البرمجية (API Reference)

يوثق هذا الدليل باللغة العربية جميع الكائنات (Structs)، الدوال (Methods)، الواجهات (Traits)، والأنماط البرمجية التي يحتاجها المطور لبناء وتطوير التطبيقات والخدمات السحابية باستخدام إطار عمل **Aether**.

---

## 📑 فهرس الكائنات والمكونات

1. [كائن التطبيق الأساسي (`AetherApp`)](#1-كائن-التطبيق-الأساسي-aetherapp)
2. [واجهة تحويل الاستجابات الموحدة (`IntoResponse`)](#2-واجهة-تحويل-الاستجابات-الموحدة-intoresponse)
3. [كائن سياق الطلب (`RequestContext`)](#3-كائن-سياق-الطلب-requestcontext)
4. [كائن بناء الاستجابة (`Response`)](#4-كائن-بناء-الاستجابة-response)
5. [مجموعات المسارات والمسارات الثابتة (`RouteGroup & Static Assets`)](#5-مجموعات-المسارات-والمسارات-الثابتة-routegroup--static-assets)
6. [كائن حدود وأمان الخادم (`ServerLimits & AetherConfig`)](#6-كائن-حدود-وأمان-الخادم-serverlimits--aetherconfig)
7. [البرمجيات الوسيطة والأمان (`Middleware & Security`)](#7-البرمجيات-الوسيطة-والأمان-middleware--security)
8. [حقن التبعيات والحالة المشتركة (`Dependency Injection & AppStateContainer`)](#8-حقن-التبعيات-والحالة-المشتركة-dependency-injection--appstatecontainer)

---

## 1. كائن التطبيق الأساسي (`AetherApp`)

كائن `AetherApp` هو المدخل الرئيسي لتسجيل المسارات، إضافة البرمجيات الوسيطة، إدارة الحالة وحقن التبعيات، وإطلاق خادم الويب.

### دوال الإنشاء والتهيئة:

#### `AetherApp::new() -> Self`
* **الوصف:** ينشئ تطبيق Aether جديد بالإعدادات الافتراضية.
* **مثال:**
  ```rust
  let mut app = AetherApp::new();
  ```

#### `AetherApp::with_config(config: AetherConfig) -> Self`
* **الوصف:** ينشئ تطبيقاً مع تخصيص شامل لحدود الذاكرة، العمال (workers)، ومسارات التشغيل.
* **مثال:**
  ```rust
  let mut config = AetherConfig::default();
  config.port = 3000;
  config.max_body_bytes = 10 * 1024 * 1024; // 10MB
  let mut app = AetherApp::with_config(config);
  ```

#### `app.manage<T: Send + Sync + 'static>(state: T) -> &mut Self`
* **الوصف:** تسجيل كائن حالة أو خدمة مشتركة داخل حاوية حقن التبعيات المتوافقة مع تعدد الخيوط ($O(1)$ TypeId DI).
* **مثال:**
  ```rust
  app.manage(DatabasePool::connect().await);
  ```

---

### دوال تسجيل المسارات المتزامنة وغير المتزامنة:

يدعم Aether تسجيل المسارات المتزامنة (Sync) فائقة السرعة للمهام البسيطة، وغير المتزامنة (Async) للمهام التي تتطلب `.await`:

#### `app.get(path, handler)` و `app.get_async(path, handler)`
* **الوصف:** تسجيل مسار لطلبات `GET`. الدالة يمكن أن تعيد أي نوع يطبق `IntoResponse` (مثل `&'static str`, `String`, `Response`, `Json<T>`, `Result<T, E>`).
* **مثال:**
  ```rust
  // متزامن (Zero-overhead Synchronous)
  app.get("/health", |_req| "OK");

  // غير متزامن (Async Handler with .await)
  app.get_async("/users/{id}", |req| async move {
      let id = req.param("id").unwrap_or("0");
      format!(r#"{{"id":"{}"}}"#, id)
  });
  ```

#### `app.post(path, handler)` و `app.post_async(path, handler)`
* **الوصف:** تسجيل مسار لطلبات `POST`.
* **مثال:**
  ```rust
  app.post_async("/api/v1/items", |req| async move {
      match req.json::<CreateItemDto>() {
          Ok(dto) => (201, "Item Created"),
          Err(e) => (400, "Invalid JSON Payload"),
      }
  });
  ```

#### `app.put(path, handler)` و `app.put_async(path, handler)`
* **الوصف:** تسجيل مسارات لطلبات `PUT` (تحديث كامل للمورد).

#### `app.delete(path, handler)` و `app.delete_async(path, handler)`
* **الوصف:** تسجيل مسارات لطلبات `DELETE` (حذف الموارد).

#### `app.patch(path, handler)` و `app.patch_async(path, handler)`
* **الوصف:** تسجيل مسارات لطلبات `PATCH` (تحديث جزئي للمورد).

---

## 2. واجهة تحويل الاستجابات الموحدة (`IntoResponse`)

تتيح واجهة `IntoResponse` للمعالجات إرجاع أنواع بيانات مرنة ومباشرة دون الحاجة لبناء كائن `Response` يدوياً في كل مرة:

```rust
pub trait IntoResponse: Send {
    fn into_response(self) -> Response;
}
```

### الأنواع المدعومة افتراضياً:
1. **`Response`**: إرجاع كائن الاستجابة الكامل مباشرة.
2. **`&'static str`**: يتم تحويله تلقائياً إلى استجابة `200 OK` بنوع `text/plain; charset=utf-8`.
3. **`String`**: يتم تحويله إلى استجابة `200 OK` نصية.
4. **`(u16, &'static str)`**: تعيين رمز الحالة والنص (مثال: `(201, "Created")` أو `(404, "Not Found")`).
5. **`(u16, String)`**: تعيين رمز الحالة مع نص ديناميكي.
6. **`Result<T, E>`**: إذا كانت النتيجة `Ok` يتم تحويل `T` إلى استجابة، وإذا كانت `Err` يتم إرجاع استجابة `400 Bad Request` بصيغة JSON تحتوي على رسالة الخطأ تلقائياً.

---

## 3. كائن سياق الطلب (`RequestContext`)

يمثل `RequestContext` كافة البيانات والمعلومات الواردة مع الطلب، مع توفير دوال مساعدة لاستخراج البيانات بسرعة وبدون حجز ذاكرة غير ضروري (Zero-Leak RAII).

### أهم الحقول (Fields):
* `ctx.path: String` - مسار الطلب بعد التنقية (مثال: `"/api/v1/users"`).
* `ctx.raw_uri: String` - الرابط الأصلي الكامل بما فيه معلمات الاستعلام.
* `ctx.query_str: Option<String>` - نص الـ Query String إن وجد.
* `ctx.method: Method` - نوع أسلوب الطلب (`Method::Get`, `Method::Post`, إلخ).
* `ctx.body: Bytes` - بايتات جسم الطلب كـ Zero-Copy slice.
* `ctx.request_id: String` - معرّف فريد لكل طلب لتتبعه (مثل `req_000000000001`).
* `ctx.headers: HashMap<String, String>` - ترويسات الطلب المنقاة.
* `ctx.params: HashMap<String, String>` - معلمات المسار الديناميكية المستخرجة.

---

### أهم دوال الاستخراج (Methods):

#### `ctx.param(key: &str) -> Option<&str>`
* **الوصف:** استخراج قيمة معلمة مسار ديناميكية (مثال: `{id}` أو `:id`).

#### `ctx.param_parse<T: FromStr>(key: &str) -> Option<T>`
* **الوصف:** استخراج وتحويل معلمة المسار مباشرة إلى نوع محدد (مثل `u64`, `i32`, `bool`).
* **مثال:**
  ```rust
  let user_id: u64 = req.param_parse("id").unwrap_or(0);
  ```

#### `ctx.query(key: &str) -> Option<&str>`
* **الوصف:** استخراج قيمة معلمة استعلام من الرابط (`?search=rust&page=1`).

#### `ctx.query_parse<T: FromStr>(key: &str) -> Option<T>`
* **الوصف:** استخراج وتحويل معلمة الاستعلام إلى نوع رقمي أو منطقي.

#### `ctx.queries() -> HashMap<&str, &str>`
* **الوصف:** استرجاع جميع معلمات الاستعلام كقاموس مفتاح-قيمة بدون استهلاك ذاكرة إضافية.

#### `ctx.cookie(name: &str) -> Option<&str>`
* **الوصف:** استخراج قيمة ملف تعريف الارتباط (Cookie) بالاسم مباشرة من ترويسة `Cookie`.
* **مثال:**
  ```rust
  let session_token = req.cookie("session_id");
  ```

#### `ctx.form_data() -> HashMap<String, String>`
* **الوصف:** فك تشفير بيانات النماذج (`application/x-www-form-urlencoded`) مباشرة من جسم الطلب مع معالجة الـ URL Decoding تلقائياً.

#### `ctx.header(name: &str) -> Option<&str>`
* **الوصف:** قراءة قيمة ترويسة معينة من ترويسات الطلب (غير حساسة لحالة الأحرف Case-Insensitive).

#### `ctx.body_str() -> Result<&str, Utf8Error>`
* **الوصف:** قراءة جسم الطلب كنص `&str` بترميز UTF-8 بدون أي عمليات نسخ للذاكرة.

#### `ctx.json<T: DeserializeOwned>() -> Result<T, serde_json::Error>`
* **الوصف:** فك تشفير جسم الطلب من صيغة JSON مباشرة إلى بنية بيانات Rust.

#### `ctx.state<T: Send + Sync + 'static>() -> Option<Arc<T>>`
* **الوصف:** استرجاع مرجع الخدمة أو الحالة المشتركة المحقونة بنظام الـ DI عبر نوعها `T`.

---

## 4. كائن بناء الاستجابة (`Response`)

يوفر كائن `Response` واجهة بانية مرنة وسريعة لإنشاء ردود HTTP بصيغ متعددة:

### دوال الإنشاء السريع (Constructors):
* `Response::ok()`: استجابة `200 OK` فارغة.
* `Response::text("محتوى نصي")`: استجابة نصية بنوع `text/plain; charset=utf-8`.
* `Response::html("<h1>عنوان</h1>")`: استجابة HTML بنوع `text/html; charset=utf-8`.
* `Response::json(bytes)`: استجابة JSON بنوع `application/json; charset=utf-8`.
* `Response::bytes(bytes, content_type)`: استجابة بايتات خام مع تحديد نوع المحتوى.
* `Response::status(code, message)`: استجابة برمز حالة مخصص.
* `Response::redirect(location, status_code)`: استجابة إعادة توجيه (301, 302, 307, 308).
* `Response::file("path/to/file.ext")`: قراءة وخدمة ملف من القرص مع تحديد الـ MIME Type تلقائياً وحماية المسارات من الاختراق (Path Traversal Protection).

### دوال التعديل والتخصيص:
* `.header("Key", "Value")`: إضافة ترويسة جديدة للاستجابة.
* `.set_cookie(name, value, http_only, secure, same_site, max_age_secs)`: إضافة ملف تعريف ارتباط آمن مع ترويسة `Set-Cookie`.

---

## 5. مجموعات المسارات والمسارات الثابتة (`RouteGroup & Static Assets`)

### تجميع المسارات (Route Grouping):
تتيح دالة `app.group` تنظيم مسارات التطبيق في مجموعات منطقية ببادئة مسار موحدة:

```rust
app.group("/api/v1", |v1| {
    v1.get("/health", |_| "v1 healthy");
    v1.get_async("/users/{id}", |req| async move {
        format!("User: {}", req.param("id").unwrap_or("0"))
    });
});
```

### خدمة الأصول الثابتة المدمجة (Embedded Static Assets):
خدمة ملفات CSS, JS, الأيقونات المضمنة في الباينري بكفاءة قصوى:

```rust
app.static_asset(
    "/static/style.css",
    "text/css",
    b"body { background: #f0f0f0; }",
);
```

---

## 6. كائن حدود وأمان الخادم (`ServerLimits & AetherConfig`)

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,      // الحد الأقصى لحجم الترويسات بالبايت (الافتراضي: 8KB)
    pub max_body_bytes: usize,        // الحد الأقصى لحجم جسم الطلب (الافتراضي: 2MB)
    pub max_headers_count: usize,     // أقصى عدد مسموح به من الترويسات (الافتراضي: 64)
    pub max_connections: usize,       // سقف الاتصالات المتزامنة المفتوحة (الافتراضي: 10,000)
    pub read_timeout: Duration,       // مهلة قراءة حزمة TCP لمنع Slowloris (الافتراضي: 5s)
    pub write_timeout: Duration,      // مهلة كتابة الاستجابة إلى المقبس (الافتراضي: 10s)
    pub keep_alive_timeout: Duration, // مهلة إبقاء المقبس مفتوحاً للطلبات التالية (الافتراضي: 60s)
}
```

---

## 7. البرمجيات الوسيطة والأمان (`Middleware & Security`)

### 1. تفعيل CORS:
```rust
app.enable_cors(CorsMiddleware::new(CorsConfig {
    allow_origin: "*",
    allow_methods: "GET, POST, PUT, DELETE, OPTIONS",
    allow_headers: "Content-Type, Authorization",
    max_age_secs: 86400,
}));
```

### 2. تفعيل ترويسات الأمان (HSTS, CSP, X-Frame-Options):
```rust
app.enable_security_headers();
```

---

## 8. حقن التبعيات والحالة المشتركة (`Dependency Injection & AppStateContainer`)

يوفر Aether نظام حقن تبعيات آمن ومتوافق مع تعدد الخيوط (Thread-Safe DI Container) معتمداً على `TypeId` دون أي انعكاس ديناميكي بطيء:

```rust
struct DbConnection {
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    // تسجيل الخدمة
    app.manage(DbConnection {
        url: "postgres://localhost/aether_db".to_string(),
    });

    // استهلاك الخدمة داخل المسار
    app.get_async("/db-info", |req| async move {
        if let Some(db) = req.state::<DbConnection>() {
            format!("Database URL: {}", db.url)
        } else {
            "Database connection not registered".to_string()
        }
    });

    app.listen("0.0.0.0:3000").await
}
```
