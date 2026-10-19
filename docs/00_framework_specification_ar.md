# دليل المواصفات والوظائف الشامل لإطار عمل Aether (بالعربية)

توثيق هندسي ومعماري شامل لكافة وظائف، كائنات، دوال، وأنظمة أمان إطار عمل **Aether** المبني بلغة Rust لتطوير خدمات الويب فائقة الأداء والأمان.

---

## 📑 فهرس الأنظمة والوظائف

1. [معمارية خط المعالجة غير المتزامن (Execution Pipeline)](#1-معمارية-خط-المعالجة-غير-المتزامن-execution-pipeline)
2. [طبقة المقابس والتأطير التراكمي لـ TCP (Adaptive Two-Tier TCP Framing)](#2-طبقة-المقابس-والتأطير-التراكمي-لـ-tcp-adaptive-two-tier-tcp-framing)
3. [محلل HTTP فائق السرعة والأمان (Zero-Copy Parser & RFC 9112)](#3-محلل-http-فائق-السرعة-والأمان-zero-copy-parser--rfc-9112)
4. [محرك التوجيه الراديكسي ومطابقة الأساليب بالـ Bitmask ($O(K)$ Router)](#4-محرك-التوجيه-الراديكسي-ومطابقة-الأساليب-بالـ-bitmask-ok-router)
5. [سياق الطلب وإدارة الذاكرة (`RequestContext`)](#5-سياق-الطلب-وإدارة-الذاكرة-requestcontext)
6. [بناء الاستجابات والتسلسل المباشر للذاكرة (`Response`)](#6-بناء-الاستجابات-والتسلسل-المباشر-للذاكرة-response)
7. [حقن التبعيات وإدارة الحالة المشتركة (`AppStateContainer` / DI DAG)](#7-حقن-التبعيات-وإدارة-الحالة-المشتركة-appstatecontainer--di-dag)
8. [منظومة البرمجيات الوسيطة (CORS، ترويسات الأمان، وتتبع الطلبات)](#8-منظومة-البرمجيات-الوسيطة-cors-ترويسات-الأمان-وتتبع-الطلبات)
9. [حدود الخادم ومكافحة هجمات Slowloris وحجب الخدمة (`ServerLimits`)](#9-حدود-الخادم-ومكافحة-هجمات-slowloris-وحجب-الخدمة-serverlimits)
10. [منظومة إدارة الأخطاء ورموز الحالة (`AetherError`)](#10-منظومة-إدارة-الأخطاء-ورموز-الحالة-aethererror)
11. [أمثلة برمجية شاملة للمطورين (Production Examples)](#11-أمثلة-برمجية-شاملة-للمطورين-production-examples)

---

## 1. معمارية خط المعالجة غير المتزامن (Execution Pipeline)

يعتمد Aether على خط معالجة مباشر غير متزامن (Fully Async Pipeline) بدون أي أقفال حابسة (Zero Global Contention):

```text
 اتصال TCP جديد من العميل
            │
            ▼
┌─────────────────────────────────────────┐
│ 1. حارس التزامن الذري (Semaphore RAII)  │  -> تحديد سقف الاتصالات (افتراضياً 10,000)
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 2. بافر القراءة التراكمي (BytesMut)     │  -> قراءة مدمجة مع مهلة Read Timeout
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 3. محلل الطلب بدون نسخ (RequestView)    │  -> مطابقة RFC 9112 وحظر Request Smuggling
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 4. شجرة التوجيه الراديكسية ($O(K)$)     │  -> مطابقة الأسلوب بالـ Bitmask واستخراج المعلمات
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 5. تنفيذ البرمجيات الوسيطة (Middleware) │  -> CORS، ترويسات الأمان، ونظام التتبع
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 6. تنفيذ معالج المسار (Async Handler)   │  -> كود التطبيق المطور باستخدام RequestContext
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 7. تحويل الاستجابة مباشرة للبافر        │  -> write_http1_to بدون تخصيص Strings وسيطة
└───────────────────┬─────────────────────┘
                    ▼
┌─────────────────────────────────────────┐
│ 8. إرسال البيانات وإفراغ المقبس (Flush) │  -> مطبق مع Write Timeout وتحرير فوري للذاكرة
└─────────────────────────────────────────┘
```

---

## 2. طبقة المقابس والتأطير التراكمي لـ TCP (Adaptive Two-Tier TCP Framing)

### طبيعة التحدي في شبكات TCP:
بيانات طلبات الـ HTTP قد تصل مجزأة عبر حزم (Packets) متعددة، أو قد تصل عدة طلبات معاً في دفعة واحدة (HTTP Pipelining).

### الحل الهندسي المعتمد:
- **المسار السريع (Fast-Path Stack Buffer):** قراءة سريعة في مصفوفة 8KB على الـ Stack.
- **البافر التراكمي المستمر (`BytesMut`):** إذا كانت البيانات جزئية (`Ok(None)`)، يتم الاحتفاظ بها في `conn_buf` وانتظار الحزمة التالية مع مراقبة `read_timeout`.
- **الاقتطاع الذري (`split_to`):** عند اكتمال طلب بحجم $N$ بايت، يتم اقتطاع الحجم بدقة $O(1)$ دون نسخ، وإعادة فحص الباقي فوراً للطلبات التالية في وضع Keep-Alive.

---

## 3. محلل HTTP فائق السرعة والأمان (Zero-Copy Parser & RFC 9112)

يقوم `RequestView` باستعارة بايتات الطلب ونصوص الترويسات مباشرة من بافر المقبس بدون أي تخصيص على الـ Heap:

### آليات الحماية الصارمة (RFC 9112):
1. **منع هجوم تكرار Content-Length (Dual CL):** رفض أي طلب يحتوي على أكثر من ترويسة أو قيم مفصولة بفواصل فوراً بـ `400 Bad Request`.
2. **منع هجوم الجمع بين Transfer-Encoding و Content-Length (CL.TE / TE.CL):** حظر فوري لأي محاولة دمج لضمان عدم اختراق الـ Reverse Proxies.
3. **التحقق من الحدود الحجمية:** فحص صارم لحجم الترويسات ضد `max_header_bytes` وحجم الجسم ضد `max_body_bytes`.

---

## 4. محرك التوجيه الراديكسي ومطابقة الأساليب بالـ Bitmask ($O(K)$ Router)

### الخوارزمية وهيكل البيانات:
- **Radix Trie مع ترتيب العقد:** تخزين المسارات كشجرة مقاطع مفروزة حتمياً:
  1. **المسارات الثابتة (Static)** أولاً (مثل `/users/profile`).
  2. **المعلمات المتغيرة (Parameterized)** ثانياً (مثل `/users/:id`).
  3. **المسارات الشاملة (Wildcard / Catch-All)** أخيراً (مثل `/static/*file`).
- **تجزئة بدون Heap Allocations:** استخدام `SmallVec<[&str; 8]>` على الـ Stack لمطابقة مقاطع المسار في $O(K)$ حيث $K$ هو عمق المسار فقط.
- **مطابقة أسلوب الطلب بتعليمة معالج واحدة (64-bit Bitmask):**
  ```rust
  if (node.allowed_methods_mask & (1 << method_id)) != 0 {
      // الأسلوب مدعوم بـ Single CPU Cycle!
  }
  ```

---

## 5. سياق الطلب وإدارة الذاكرة (`RequestContext`)

كائن `RequestContext` يوفر للمطور واجهة نظيفة مع إدارة ذاكرة منضبطة (تم إلغاء `Box::leak` بالكامل لضمان عدم حدوث أي تسريب ذاكرة):

### دوال الاستخراج المتاحة:

| الدالة | التوقيع البرمجي (Signature) | الوظيفة |
| :--- | :--- | :--- |
| `param` | `pub fn param(&self, key: &str) -> Option<&str>` | استخراج قيمة معلمة مسار (مثل `:id`). |
| `param_parse` | `pub fn param_parse<T: FromStr>(&self, key: &str) -> Option<T>` | استخراج وتحويل معلمة المسار لنوع محدد (`u32`, `bool`, إلخ). |
| `query` | `pub fn query(&self, key: &str) -> Option<&str>` | استخراج معلمة من الـ Query String (`?page=2`). |
| `header` | `pub fn header(&self, name: &str) -> Option<&str>` | قراءة قيمة ترويسة معينة (غير حساسة لحالة الأحرف). |
| `cookie` | `pub fn cookie(&self, name: &str) -> Option<&str>` | استخراج قيمة Cookie معينة من ترويسة `Cookie`. |
| `body_str` | `pub fn body_str(&self) -> Result<&str, Utf8Error>` | قراءة جسم الطلب كنص UTF-8 بدون نسخ. |
| `json` | `pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error>` | فك تشفير JSON مباشرة من بايتات الجسم إلى Struct. |
| `state` | `pub fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>>` | استرجاع خدمة أو حالة مشتركة من حاوية الـ DI. |
| `request_id` | `pub fn request_id(&self) -> &str` | الحصول على معرّف التتبع الفريد للطلب. |

---

## 6. بناء الاستجابات والتسلسل المباشر للذاكرة (`Response`)

### دوال إنشاء الاستجابات:
- `Response::ok(body)`: إرجاع استجابة بحالة `200 OK`.
- `Response::json(&data)`: تحويل الهيكل إلى JSON تلقائياً مع ترويسة `Content-Type: application/json`.
- `Response::status(code, text)`: إنشاء استجابة برمز حالة مخصص (مثل `201 Created` أو `404 Not Found`).

### التحويل المباشر للبافر (`write_http1_to`):
تتم كتابة الترويسات وجسم الاستجابة مباشرة إلى الـ `BytesMut` الخاص بالمقبس عبر `bytes::BufMut::writer` بدون إنشاء أي `String` مؤقتة أو استدعاء `format!()` على الـ Heap.

---

## 7. حقن التبعيات وإدارة الحالة المشتركة (`AppStateContainer` / DI DAG)

يوفر Aether حاوية حقن تبعيات آمنة للـ Threads تعتمد على `TypeId` لاسترجاع الخدمات في $O(1)$:

```rust
let mut app = AetherApp::new();

// تسجيل الخدمات
app.state_container.insert(DatabaseConnectionPool::new());
app.state_container.insert(RedisCacheClient::new());

// استدعاؤها داخل أي Handler
app.get("/items", |ctx: RequestContext| async move {
    let db = ctx.state::<DatabaseConnectionPool>().unwrap();
    Response::ok("Items fetched")
});
```

---

## 8. منظومة البرمجيات الوسيطة (Middleware)

- **`CorsMiddleware`**: ضبط سياسات النطاقات المسموحة، الأساليب، والترويسات.
- **`SecurityHeadersMiddleware`**: تفعيل ترويسات الحماية (HSTS, Frame-Options `DENY`, Content-Type `nosniff`, CSP).
- **`LoggerMiddleware`**: مبني بتجريد صفري التكلفة (`#[inline(always)]`) ومعطل افتراضياً على الـ Hot Path لمنع خنق الأداء بسبب أقفال `stdout` العامة.

---

## 9. حدود الخادم ومكافحة هجمات Slowloris وحجب الخدمة (`ServerLimits`)

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,     // الحد الأقصى للترويسات (8 KB افتراضياً)
    pub max_body_bytes: usize,       // الحد الأقصى للجسم (2 MB افتراضياً)
    pub max_headers_count: usize,    // الحد الأقصى لعدد الترويسات (64 ترويسة)
    pub max_connections: usize,      // سقف الاتصالات المتزامنة (10,000 اتصال)
    pub read_timeout: Duration,      // مهلة قراءة الحزمة (5 ثوانٍ لمكافحة Slowloris)
    pub write_timeout: Duration,     // مهلة كتابة الاستجابة (10 ثوانٍ)
    pub keep_alive_timeout: Duration,// مهلة إبقاء الاتصال (60 ثانية)
}
```

---

## 10. منظومة إدارة الأخطاء ورموز الحالة (`AetherError`)

- `AetherError::MalformedRequest` -> ينتج `400 Bad Request`.
- `AetherError::HeaderTooLarge` -> ينتج `431 Request Header Fields Too Large`.
- `AetherError::PayloadTooLarge` -> ينتج `413 Payload Too Large`.
- `AetherError::Timeout` -> إغلاق آمن للمقبس وتحرير الموارد.

---

## 11. أمثلة برمجية شاملة للمطورين (Production Examples)

```rust
use aether::app::AetherApp;
use aether::context::RequestContext;
use http::response::Response;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Product {
    id: u64,
    name: String,
    price: f64,
}

#[derive(Deserialize)]
struct CreateProductDto {
    name: String,
    price: f64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = AetherApp::new();

    // 1. مسار استعلام بسيط
    app.get("/", |_ctx| async move {
        Response::ok("محرك Aether يعمل بكفاءة قصوى ⚡")
    });

    // 2. مسار ديناميكي مع معلمة مسار
    app.get("/products/:id", |ctx: RequestContext| async move {
        let product_id = ctx.param_parse::<u64>("id").unwrap_or(0);
        Response::json(&Product {
            id: product_id,
            name: format!("Product #{}", product_id),
            price: 49.99,
        })
    });

    // 3. مسار استقبال وفك تشفير JSON
    app.post("/products", |ctx: RequestContext| async move {
        match ctx.json::<CreateProductDto>() {
            Ok(dto) => Response::status(201, "Created").with_json(&Product {
                id: 101,
                name: dto.name,
                price: dto.price,
            }),
            Err(_) => Response::status(400, "Bad Request: Invalid JSON Body"),
        }
    });

    println!("🚀 الخادم يعمل على http://0.0.0.0:3000");
    app.listen("0.0.0.0:3000").await?;
    Ok(())
}
```
