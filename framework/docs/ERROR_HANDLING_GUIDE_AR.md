# دليل معالجة الأخطاء والتشخيص في إطار عمل Aether ⚡

> **ملاحظة تدقيق (2026-10-18):** لافتة الخادم الواردة أدناه قديمة. اللافتة الحالية تصرّح بأن
> المشغّل هو Tokio متعدد الخيوط (work-stealing) وأن المخصص هو مخصص النظام، وليس
> Thread-per-Core أو RequestArena. راجع
> [PRODUCTION_READINESS_REPORT.md](PRODUCTION_READINESS_REPORT.md).

### (Error Handling, Diagnostic Engine & Terminal UX Guide)

يقدم **Aether** نظام تشخيص أخطاء متطور مستوحى من أفضل ممارسات هندسة الأنظمة (System Diagnostics)، مصمم لتزويد المطور بإجابات فورية وقابلة للتنفيذ عند حدوث أي خطأ، دون التضحية بأداء الإنتاج أو تسريب البيانات الحساسة.

---

## 📑 الفهرس
1. [جدول أكواد الأخطاء القياسية (Standard Error Codes: E1001 - E5002)](#1-جدول-أكواد-الأخطاء-القياسية)
2. [تصميم مخرجات الطرفية (Terminal Diagnostic UX)](#2-تصميم-مخرجات-الطرفية)
3. [معالجة الأخطاء في وضع التطوير مقابل الإنتاج (Dev vs Prod)](#3-معالجة-الأخطاء-في-وضع-التطوير-مقابل-الإنتاج)
4. [معرف التتبع للطلبات (Request ID & Trace Correlation)](#4-معرف-التتبع-للطلبات)
5. [أمثلة عملية لحل المشاكل الشائعة (Troubleshooting Recipes)](#5-أمثلة-عملية-لحل-المشاكل-الشائعة)

---

<a name="1-جدول-أكواد-الأخطاء-القياسية"></a>
## 1. جدول أكواد الأخطاء القياسية (Standard Error Codes)

| كود الخطأ | العنوان | السبب الشائع | الحل المقترح |
| :--- | :--- | :--- | :--- |
| **`E1001`** | `ServerStartupFailed` | المنفذ (Port) محجوز بواسطة تطبيق آخر أو عدم وجود صلاحيات | تغيير المنفذ عبر `AETHER_PORT` أو إيقاف العملية الحاجزة |
| **`E1002`** | `InvalidConfig` | قيمة متغير بيئة غير صالحة (مثل منفذ غير رقمي) | تصحيح قيمة المتغير في ملف `.env` أو إعدادات النظام |
| **`E2001`** | `RouteCollision` | تعارض مسارين بنفس النمط في شجرة الـ Radix | إعادة تسمية المسار المتعارض أو تنظيم المسارات في `group` |
| **`E2002`** | `RouteNotFound` | تم طلب مسار غير مسجل في السيرفر (HTTP 404) | التحقق من صحة رابط الـ URL أو إضافة مسار مخصص |
| **`E2003`** | `MethodNotAllowed` | الفعل المستخدم (مثل `POST`) غير مفعل للمسار (HTTP 405) | تفعيل الفعل المطلوب عبر `app.post(...)` أو تعديل طلب العميل |
| **`E3001`** | `MalformedRequest` | طلب HTTP غير متوافق مع قواعد RFC 9112 أو محاولة تهريب (Smuggling) | التحقق من سلامة عميل HTTP والتأكد من إرسال ترويسات قياسية |
| **`E3002`** | `PayloadLimitExceeded` | حجم الترويسات أو جسم الطلب تجاوز الحد المسموح | زيادة `max_body_bytes` في `AetherConfig` |
| **`E4001`** | `DeserializationFailed` | حقول الـ JSON لا تتطابق مع بنية الـ Struct في Rust | مطابقة أسماء وأنواع الحقول في الـ Struct مع الـ Payload |
| **`E4002`** | `ValidationFailed` | فشل في قيود التحقق من صحة المدخلات (SIMD Validator) | تصحيح البيانات المدخلة لتوافق الشروط المطلوبة |
| **`E5001`** | `DependencyCycle` | حلقة مغلقة دائرية في شجرة حقن التبعيات (DAG) | فك الاعتمادية الدائرية بين الخدمات في الـ Graph |
| **`E5002`** | `InternalServerError` | خطأ غير معالج داخل دالة الـ Handler (HTTP 500) | استخدام `Result<T, E>` لإرجاع الأخطاء بشكل منظم |

---

<a name="2-تصميم-مخرجات-الطرفية"></a>
## 2. تصميم مخرجات الطرفية (Terminal Diagnostic UX)

### أ) لافتة إقلاع السيرفر (Server Startup Banner):
عند تشغيل التطبيق، يظهر ملخص واضح لمعلمات الخادم:
```text
┌─────────────────────────────────────────────────────────────┐
│ ⚡ AETHER WEB FRAMEWORK (Rust Native Zero-Copy Engine)       │
├─────────────────────────────────────────────────────────────┤
│ Environment : development                                   │
│ Address     : http://127.0.0.1:3000                         │
│ Routes      : 14                                            │
│ Workers     : 8 (Thread-per-Core Async Epoll)               │
│ Allocator   : RequestArena (O(1) Bump Reset - Zero Heap)    │
└─────────────────────────────────────────────────────────────┘
```

### ب) رسالة الخطأ التفاعلية عند حجز المنفذ (`E1001`):
```text
ERROR [E1001] Server startup failed
  Address: 0.0.0.0:3000
  Reason : Address already in use (os error 98)

  Hint:
  Another process is actively listening on port 3000.

  Try:
  • Stop conflicting process: lsof -ti:3000 | xargs kill -9
  • Change port via AETHER_PORT environment variable (e.g. AETHER_PORT=3001)
```

---

<a name="3-معالجة-الأخطاء-في-وضع-التطوير-مقابل-الإنتاج"></a>
## 3. معالجة الأخطاء في وضع التطوير مقابل الإنتاج (Dev vs Prod)

- **في وضع التطوير (`development`):**
  - يتم إظهار تفاصيل الخطأ وسياقه ورقم السطر للمطور لتسريع عملية تصحيح الكود.
- **في وضع الإنتاج (`production`):**
  - يتم إخفاء مسارات الملفات الداخلية، وحجب كلمات المرور، ومفاتيح الـ API، والـ Tokens المشفرة من السجلات، مع إرجاع استجابة JSON آمنة للعميل تحتوي فقط على `code` و `error_id`.

---

<a name="4-معرف-التتبع-للطلبات"></a>
## 4. معرف التتبع للطلبات (Request ID & Trace Correlation)

يقوم Aether تلقائياً بتعيين معرف فريد لكل طلب في ترويسة `X-Request-ID`:
```http
HTTP/1.1 200 OK
Content-Type: application/json; charset=utf-8
X-Request-ID: req_000000000042
```
يمكنك قراءة هذا المعرف واستخدامه في الـ Handlers عبر:
```rust
app.get("/orders", |req| {
    let trace_id = req.request_id();
    format!(r#"{{"trace_id":"{}"}}"#, trace_id)
});
```

---

<a name="5-أمثلة-عملية-لحل-المشاكل-الشائعة"></a>
## 5. أمثلة عملية لحل المشاكل الشائعة (Troubleshooting Recipes)

### 1. معالجة فشل فك الـ JSON بشكل أنيق:
```rust
app.post_async("/api/checkout", |req| async move {
    let payload: CheckoutDto = match req.json() {
        Ok(data) => data,
        Err(err) => {
            return Response::json(bytes::Bytes::from(format!(
                r#"{{"error":"Invalid JSON Payload: {}","code":"E4001"}}"#,
                err
            )))
            .header("Content-Type", "application/json");
        }
    };
    Response::ok()
});
```

### 2. إعادة التوجيه الآمن (Redirects):
```rust
app.get("/old-docs", |_req| {
    Response::redirect("/docs/v2", 301)
});
```
