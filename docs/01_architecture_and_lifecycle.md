# 01. نظرة عامة على المعمارية ودورة حياة الطلب (Architecture & Request Lifecycle)

تم بناء Aether بنظام طبقات معيارية تفصل بين عمليات القراءة من الشبكة، والتحليل (Parsing)، والتوجيه (Routing)، والتنفيذ (Handler Execution).

---

## 1. مخطط تدفق دورة حياة الطلب (Request Lifecycle Flow)

```text
       Incoming TCP Stream (Client)
                   │
                   ▼
┌─────────────────────────────────────────┐
│     1. Connection Semaphore Check       │  (Limits: max_connections = 10,000)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│  2. Persistent Buffering (BytesMut)     │  (Two-tier read with read_timeout)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│      3. RequestView Parser & Security   │  (RFC 9112, No Smuggling, Zero-Copy)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│      4. Radix Trie Router ($O(K)$)      │  (Bitmask Method Match, Path Params)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│  5. Middleware Pipeline & Context Init  │  (CORS, Security Headers, Tracing)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│         6. Async Handler Execution      │  (JSON / Text / Custom Response)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│   7. Direct Buffer Serialization        │  (write_http1_to direct to buffer)
└──────────────────┬──────────────────────┘
                   ▼
┌─────────────────────────────────────────┐
│    8. Async TCP Write & Arena Reset     │  (write_timeout applied, buffer split)
└─────────────────────────────────────────┘
```

---

## 2. الصناديق البرمجية الأساسية (Crates Hierarchy)

- **`aether`**: المحرك الأساسي والخادم الذي يدمج جميع المكونات ويشغل الـ Event Loop مع Tokio.
- **`http`**: تراكيب البيانات ونماذج HTTP (الترويسات، الـ Parser، حدود الأمان `ServerLimits`، والـ Responses).
- **`router`**: محرك التوجيه الراديكسي (Radix Trie) مع دعم المعلمات الديناميكية و الـ Bitmask للـ Methods.
- **`middleware`**: معالجات الأمان، CORS، وتسجيل السجلات الموجه للأداء الفائق.
- **`observability`**: أدوات قياس الأداء وتتبع الطلبات (Metrics & Tracing).
