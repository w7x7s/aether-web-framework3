# 05. السياق وتفادي الـ Allocations (RequestContext & Memory Management)

تمت إعادة هندسة كائن سياق الطلب `RequestContext` في Aether لإلغاء أي تسريب للذاكرة (Zero Memory Leaks) وتقليل الحجوزات غير الضرورية.

---

## 1. التخلص التام من `Box::leak`

### المشكلة السابقة:
كانت النسخ الأولية تستخدم `Box::leak(path.into_boxed_str())` لتحويل السلاسل النصية إلى `&'static str` لإرضاء الـ Borrow Checker أثناء التنفيذ غير المتزامن (`tokio::spawn`).
- **النتيجة الكارثية:** كل طلب وارد كان يقتطع جزءاً دائماً من الذاكرة (Memory Leak) لا يتحرر أبداً، مما يسبب امتلاء الرام وانهيار الخادم تحت الضغط العالي.

### الحل المطبق:
تم استبدال ذلك بملكية واضحة ومباشرة:
- النصوص تُدار كـ `String` منضبطة.
- البايتات تُدار كـ `Bytes` (التي توفر Zero-Copy slicing ومشاركة مؤشرات الذاكرة بحساب مراجع ذري ذرّي دون نسخ البايتات).
- فور انتهاء الـ Handler، يتم تحرير كافة هياكل الـ `RequestContext` تلقائياً عبر آلية الـ RAII في Rust.

---

## 2. بنية الـ `RequestContext`

```rust
pub struct RequestContext {
    pub raw_uri: String,
    pub path: String,
    pub query_str: Option<String>,
    pub method: Method,
    pub params: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Bytes,
    pub request_id: String,
    pub state: Arc<AppState>,
}

impl RequestContext {
    /// استخراج جسم الطلب كنص UTF-8 بدون نسخ
    pub fn body_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }

    /// فك تشفير JSON مباشرة من بايتات الطلب
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }

    /// قراءة معلمة مسار بسهولة
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }

    /// قراءة ترويسة
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|s| s.as_str())
    }
}
```

---

## 3. تحسين تسلسل الاستجابة (Direct Buffer Serialization)

عند تجهيز الـ Response، تم استبدال استخدام الـ `format!()` والـ `String` الوسيطة بكتابة مباشرة إلى الـ `BytesMut` الخاص بالمقبس:

```rust
pub fn write_http1_to(&self, out: &mut BytesMut) {
    use std::io::Write;
    let mut writer = bytes::BufMut::writer(out);
    let _ = write!(writer, "HTTP/1.1 {} {}\r\n", self.status, self.status_text);

    for (k, v) in &self.headers {
        let _ = write!(writer, "{}: {}\r\n", k, v);
    }
    let _ = write!(writer, "\r\n");

    drop(writer);
    out.extend_from_slice(&self.body);
}
```
**الفائدة:** صفر Heap Allocations وسيطة لكتابة ترويسات الاستجابة.
