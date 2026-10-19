# 04. معالجة الترويسات وحماية الـ Request Smuggling (RFC 9112 Security)

يلتزم Aether بصرامة بمتطلبات مواصفات RFC 9112 لحماية الخادم والتطبيقات المبنية عليه من هجمات Desync و HTTP Request Smuggling.

---

## 1. حماية الترويسات (Content-Length & Transfer-Encoding)

تحدث هجمات الـ Request Smuggling عندما يختلف الخادم الأمامي (Reverse Proxy / Nginx / Cloudflare) عن الخادم الخلفي في تحديد نهاية الطلب.

### القواعد الأمنية المطبقة في Aether:

1. **رفض تكرار ترويسة `Content-Length` (Dual CL Attack):**
   - إذا تضمن الطلب أكثر من ترويسة `Content-Length`، أو ترويسة بقيم متعددة (مثل `Content-Length: 10, 20`)، يتم رفض الطلب فوراً بـ `400 Bad Request`.

2. **رفض دمج `Transfer-Encoding` مع `Content-Length` (CL.TE / TE.CL Attack):**
   - وفقاً لـ RFC 9112 بند 6.3، وجود كلا الترويستين معاً يُعد دليلاً على محاولة تلاعب أمني. يرفض Aether هذا الطلب مباشرة.

3. **التحقق من صحة القيمة الرقمية:**
   - رفض القيم السالبة أو غير الرقمية أو القيم التي تتجاوز السعة القصوى للـ `usize`.

---

## 2. كود التحقق الأمني في الـ Parser (`parser.rs`)

```rust
let mut content_length: Option<usize> = None;
let mut has_transfer_encoding = false;

for h in headers.iter() {
    if h.name.eq_ignore_ascii_case("content-length") {
        if content_length.is_some() {
            // ثغرة Smuggling محتملة: تم تكرار Content-Length
            return Err(AetherError::MultipleContentLength);
        }
        let val_str = std::str::from_utf8(h.value).map_err(|_| AetherError::InvalidHeader)?;
        let parsed_len: usize = val_str.trim().parse().map_err(|_| AetherError::InvalidContentLength)?;
        content_length = Some(parsed_len);
    } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
        has_transfer_encoding = true;
    }
}

// RFC 9112: حظر دمج Transfer-Encoding مع Content-Length
if content_length.is_some() && has_transfer_encoding {
    return Err(AetherError::ConflictingLengthHeaders);
}
```

---

## 3. الأمان التشفيري في الـ JWT (Constant-Time Verification)

عند التحقق من تواقيع JWT أو رموز الأمان:
- لا يتم استخدام مقارنة السلاسل النصية القياسية `==` (والتي تخرج مبكراً عند أول بايت غير متطابق مسببة تسريب توقيت - Timing Attack).
- يتم استخدام دالة ثابتة التوقيت `subtle::ConstantTimeEq` لضمان أن وقت المعالجة متطابق تماماً بغض النظر عن صحة التوقيع.
