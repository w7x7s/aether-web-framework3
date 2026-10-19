# 07. الحدود، المهلات، والأمان ضد هجمات الـ DoS (Limits & Slowloris Mitigation)

لحماية الخادم من هجمات استنزاف الموارد (Resource Exhaustion) والاتصالات المعلقة، يوفر Aether هيكل `ServerLimits` مع تطبيق فعلي على مستوى المقابس (Sockets).

---

## 1. بنية `ServerLimits`

```rust
pub struct ServerLimits {
    pub max_header_bytes: usize,     // الحد الأقصى لحجم الترويسات (افتراضياً 8 KB)
    pub max_body_bytes: usize,       // الحد الأقصى لحجم جسم الطلب (افتراضياً 2 MB)
    pub max_headers_count: usize,    // الحد الأقصى لعدد الترويسات (افتراضياً 64)
    pub max_connections: usize,      // الحد الأقصى للاتصالات المتزامنة (افتراضياً 10,000)
    pub read_timeout: Duration,      // مهلة قراءة حزمة TCP (افتراضياً 5 ثوانٍ)
    pub write_timeout: Duration,     // مهلة كتابة الاستجابة (افتراضياً 10 ثوانٍ)
    pub keep_alive_timeout: Duration,// مهلة الحفاظ على الاتصال (افتراضياً 60 ثانية)
}
```

---

## 2. آليات الحماية المطبقة

### 1. الحماية من هجمات Slowloris (Read Timeout):
يتم تغليف كل عملية قراءة بمهلة زمنية:
```rust
let read_result = tokio::time::timeout(
    limits.read_timeout,
    stream.read(&mut temp_buf),
).await;
```
إذا حاول المهاجم فتح اتصال وإرسال بايت واحد كل دقيقة لإبقاء المقبس مفتوحاً، يتم إسقاط الاتصال فور انتهاء الـ 5 ثوانٍ وتحرير الموارد.

### 2. الحماية من بطء العميل (Write Timeout):
يتم تغليف كتابة الاستجابة أيضاً بمهلة `write_timeout` لمنع العميل البطيء من احتجاز الـ Worker Task.

### 3. سقف الاتصالات المتزامنة (Connection Limiting via Semaphore):
يتم تقييد حلقة الاستقبال `accept()` بـ `tokio::sync::Semaphore`:
```rust
let permit = conn_semaphore.clone().acquire_owned().await.unwrap();
// يتم تمرير الـ permit لداخل الـ tokio task
// عند إغلاق الاتصال أو انتهائه، يتحرر الـ permit تلقائياً عبر Drop
```
يمنع هذا فتح أكثر من `max_connections` (10,000 افتراضياً) مما يحمي من هجمات الـ File Descriptor Exhaustion.

### 4. التحكم الصارم بأحجام الذاكرة:
إذا تجاوزت الترويسات 8 KB، يتم الرد بـ `431 Request Header Fields Too Large`.  
إذا تجاوز الجسم 2 MB، يتم الرد بـ `413 Payload Too Large`.
