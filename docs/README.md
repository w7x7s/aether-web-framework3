# Aether Web Framework — Developer & Architectural Documentation

مرحباً بك في التوثيق الهندسي الشامل لإطار عمل **Aether** المكتوب بلغة Rust.
تم تصميم Aether ليكون إطار عمل فائق السرعة، خالي من الـ Allocations غير الضرورية (Near Zero-Copy)، ومتوافقاً بشكل صارم مع معايير الأمان وشبكات الويب الحديثة (RFC 9112).

---

## فهرس المحتويات (Table of Contents)

1. [📑 الدليل الشامل والمواصفات الكاملة لكافة الوظائف (بالعربية)](./00_framework_specification_ar.md)
2. [📑 Complete Comprehensive Framework Specification (English)](./00_framework_specification_en.md)
3. [نظرة عامة على المعمارية (Architecture Overview)](./01_architecture_and_lifecycle.md)
4. [معالجة شبكة TCP والتأطير (TCP Framing & Connection Buffering)](./02_tcp_framing_and_buffering.md)
5. [محرك التوجيه وخوارزمية الـ Radix Trie (Routing Engine)](./03_routing_and_radix_trie.md)
6. [معالجة الترويسات وحماية الـ Request Smuggling (RFC 9112 Security)](./04_security_and_rfc9112.md)
7. [السياق وتفادي الـ Allocations (RequestContext & Memory Management)](./05_request_context_and_memory.md)
8. [البرمجيات الوسيطة والأداء (Middleware & Hot-Path Optimization)](./06_middleware_and_performance.md)
9. [الحدود، المهلات، والأمان ضد هجمات الـ DoS (Limits & Slowloris Mitigation)](./07_limits_and_dos_mitigation.md)
10. [دليل البدء السريع وأمثلة الاستخدام (Getting Started & Examples)](./08_getting_started_and_examples.md)
11. [المرجع الشامل لدوال وكائنات Aether باللغة العربية (API Reference)](./09_api_reference.md)
12. [تقرير التحول الهندسي ودراسة القدرات التنافسية (بالعربية)](./10_competitive_engineering_report_ar.md)
13. [Comprehensive Engineering Transformation & Capability Report (English)](./10_competitive_engineering_report_en.md)

---

## المعايير الأساسية لفريق التطوير (Core Engineering Principles)

$$\text{Correctness} \longrightarrow \text{Security} \longrightarrow \text{Predictability} \longrightarrow \text{Measured Performance} \longrightarrow \text{Maintainability}$$

- **Zero Global Contention:** لا نستخدم أي أقفال عامة (`Mutex` أو قفل `stdout` عبر `println!`) في الـ Hot Path.
- **Strict RFC Conformance:** رفض أي طلب يخالف معايير بروتوكول HTTP أو يحتوي على علامات تلاعب (Request Smuggling / Path Traversal).
- **Stack-First Allocations:** تفضيل التخصيص على الـ Stack عبر `SmallVec` بدلاً من الـ Heap كلما أمكن.
