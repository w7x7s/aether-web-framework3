# 02. معالجة شبكة TCP والتأطير (TCP Framing & Connection Buffering)

في بروتوكول TCP، قد تصل بيانات طلب HTTP مجزأة عبر حزم (Segments) متعددة، أو قد تصل عدة طلبات متتالية ضمن قراءة شبكية واحدة (HTTP Pipelining / Keep-Alive).

---

## 1. مشكلة التجزئة والحل الهندسي في Aether

### المشكلة السابقة (Naïve Fixed Buffer):
قراءة البيانات في مصفوفة `[u8; 8192]` وفصلها عن القراءات التالية كان يؤدي إلى:
1. فشل قراءة طلبات `POST` أو الطلبات ذات الترويسات الكبيرة بـ `400 Bad Request` عند وصولها عبر أكثر من TCP packet.
2. فقدان البيانات المتبقية عند وجود طلب ثانٍ في نفس القراءة.

### الحل المطبق (Persistent Two-Tier Framing):

```text
TCP Socket
   │
   ▼
┌───────────────────────┐
│ Temporary Read Chunk  │ (8KB buffer via stream.read)
└───────────┬───────────┘
            ▼
┌───────────────────────┐
│ Connection Buffer     │ (BytesMut - accumulates stream chunks)
└───────────┬───────────┘
            ▼
       RequestView::parse(&conn_buf)
            │
     ┌──────┴──────┐
     ▼             ▼
 Ok(Some)       Ok(None)
(Complete)    (Partial / Fragmented)
     │             │
     │             ▼
     │      Wait for next TCP chunk (with read_timeout)
     ▼      and accumulate into conn_buf
 Process Request
     │
     ▼
conn_buf.split_to(consumed)  ──>  Advance buffer by exact bytes
     │
     ▼
Check conn_buf again (Instant loop for pipelined requests!)
```

---

## 2. إدارة الذاكرة واقتطاع البايتات (`split_to`)

باستخدام `BytesMut::split_to(consumed)`:
- يتم فصل الشريحة التي تم استهلاكها بالكامل بـ $O(1)$ ودون أي عمليات نسخ للذاكرة (Zero-Copy pointer slicing).
- البايتات المتبقية في `conn_buf` تصبح هي بداية الطلب التالي مباشرة.
- يتم تصفير الـ Arena عبر `arena.reset()` لإعادة استخدام المساحات المحجوزة فورياً.

---

## 3. كود المعالجة في الـ Runtime (`app.rs`)

```rust
// حلقة معالجة الطلبات المتتالية في نفس الاتصال
loop {
    if conn_buf.is_empty() {
        break;
    }

    match RequestView::parse(&conn_buf, &limits_clone) {
        Ok(Some((req, consumed))) => {
            // تنفيذ الطلب...
            let _ = conn_buf.split_to(consumed);
            arena.reset();
        }
        Ok(None) => {
            // الطلب غير مكتمل، ننتظر قراءة الحزمة التالية من TCP
            break;
        }
        Err(err) => {
            // إرسال كود الخطأ المناسب (431 / 413 / 400) وإغلاق المقبس
            return;
        }
    }
}
```
