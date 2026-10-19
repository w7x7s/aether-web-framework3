# 06. البرمجيات الوسيطة والأداء (Middleware & Hot-Path Optimization)

تعتبر البرمجيات الوسيطة (Middleware) جزءاً حساساً جداً للأداء لأنها تُنفذ مع كل طلب وارد.

---

## 1. مشكلة الـ Global Locks في السجلات (`println!`)

### تحليل المشكلة:
في اختبارات الضغط السابقة، بلغ الأداء ~1,635 RPS فقط رغم أن وقت معالجة الخادم الفعلي كان بين 6 إلى 25 ميكروثانية.
- **السبب الجذري:** كان الـ `LoggerMiddleware` يستدعي `println!()` مع كل طلب. في معيار Rust القياسي، `println!` تستحوذ على `std::io::stdout().lock()`.
- هذا القفل هو **Global Mutex** مشترك بين كافة الـ Threads و الـ Tokio Tasks، مما سبب طابور انتظار ضخم (Lock Contention) خنق التوازي بالكامل ورفع زمن الاستجابة إلى ~10ms.

### الحل المطبق:
1. تعطيل الطباعة على الـ Hot Path افتراضياً في بيئة الإنتاج واختبارات الـ Benchmarks.
2. إضافة ترويسة `#[inline(always)]` لضمان أن استدعاء الـ Logger لا يضيف أي Overhead في حالة التعطيل (Zero-Cost Abstraction).
3. إمكانية تفعيل السجلات في بيئة التطوير عبر:
   ```rust
   let app = AetherApp::new()
       .with_logger(LoggerMiddleware::new().enabled(true));
   ```

---

## 2. برمجية الأمان وحماية المسارات (Security & Static Files)

### منع اختراق المسارات (Path Traversal Protection):
تحتوي دالة `safe_resolve_path` على فحص صارم للمسارات الثابتة:
- حظر تسلسل `..` للرجوع للخلف.
- حظر الـ Double URL Encoding (`%252e%252e`).
- حظر حروف الـ Null Byte (`\0`).
- التأكد الحتمي من أن المسار الناتج يقع تحت المجلد الجذري المصرح به (`starts_with(base_dir)`).

---

## 3. برمجية CORS الفائقة

تسمح برمجية CORS بضبط السياسات (Allowed Origins, Methods, Headers) مع تطبيقها مباشرة على الاستجابة دون إعادة تخصيص نصوص.
