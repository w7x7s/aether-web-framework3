# 03. محرك التوجيه وخوارزمية الـ Radix Trie (Routing Engine)

تم تصميم محرك التوجيه في Aether لتحقيق سرعة بحث متناهية $O(K)$ حيث $K$ هو عدد مقاطع المسار، مع تقليل التخصيصات في الذاكرة (Allocations) إلى الصفر في المسارات العادية.

---

## 1. شجرة التوجيه الراديكسية (Radix Trie Architecture)

يتم تخزين المسارات في شجرة مقاطع (`RouteNode`) بدلاً من البحث الخطي:

```text
               / (Root)
             ┌────┴─────────┐
           api            static (CatchAll: *filepath)
         ┌───┴───┐
       v1       v2
     ┌───┴───┐
   users   posts
     │
   :id (Param)
     │
   orders
```

---

## 2. أسبقية مطابقة المسارات الحتمية (Deterministic Route Precedence)

لضمان عدم وجود أي غموض في مطابقة الروابط، يتبع الـ Router الترتيب الصارم التالي:

1. **المسارات الثابتة أولاً (Exact Static Match)**: مثل `/users/me`
2. **المعلمات الديناميكية ثانياً (Parameterized Match)**: مثل `/users/:id` أو `/users/{id}`
3. **المسارات الشاملة ثالثاً (Wildcard / Catch-All)**: مثل `/static/*filepath`

---

## 3. مطابقة أساليب HTTP عبر الـ Bitmask (Single-Instruction Method Matching)

بدلاً من مقارنة النصوص أو البحث في مصفوفات، يتم تمثيل كل أسلوب HTTP بـ Bit في حقل `u64`:

```rust
pub const METHOD_GET: u64     = 1 << 0;
pub const METHOD_POST: u64    = 1 << 1;
pub const METHOD_PUT: u64     = 1 << 2;
pub const METHOD_DELETE: u64  = 1 << 3;
pub const METHOD_PATCH: u64   = 1 << 4;
pub const METHOD_HEAD: u64    = 1 << 5;
pub const METHOD_OPTIONS: u64 = 1 << 6;
```

**الميزة:**
- فحص ما إذا كان المسار يدعم الأسلوب المطلوب يتم بتعليمة معالج واحدة (Single CPU Cycle):
  ```rust
  if (node.allowed_methods_mask & (1 << method_id)) != 0 {
      // Method is supported!
  }
  ```
- إرجاع خطأ `405 Method Not Allowed` فوراً مع قائمة الأساليب المسموحة بدقة.

---

## 4. تقليل الـ Allocations باستخدام `SmallVec`

عند استقبال مسار مثل `/api/v1/users/42`، يتم تقسيم المسار باستخدام:
```rust
let mut segments: SmallVec<[&str; 8]> = SmallVec::new();
```
- **0 Heap Allocations:** للمسارات التي تحتوي على أقل من 8 مقاطع (أكثر من 99.9% من مسارات الويب الحقيقية)، يتم حجز السلاسل بالكامل على الـ CPU Stack وتفادي الـ Heap Allocator تماماً.
