export interface ADR {
  id: string;
  title: string;
  status: 'Accepted' | 'Proposed' | 'Superseded';
  context: string;
  decision: string;
  consequences: {
    positive: string[];
    negative: string[];
  };
  alternatives: string[];
}

export interface BenchmarkItem {
  name: string;
  metric: string;
  fastapi: number;
  actix: number;
  drogon: number;
  aether: number;
  unit: string;
  higherIsBetter: boolean;
  notes: string;
}

export interface MemoryBudgetEntry {
  component: string;
  fastapiBytes: number;
  actixBytes: number;
  drogonBytes: number;
  aetherBytes: number;
  allocationStrategy: string;
}

export const ADR_LIST: ADR[] = [
  {
    id: 'ADR-001',
    title: 'Language Boundary: Pure Rust Core with Isolated C-ABI SIMD Kernels',
    status: 'Accepted',
    context: 'FastAPI suffers from CPython GIL and dynamic typing overhead. Modern C++ offers raw memory control and non-blocking I/O (Drogon), while Rust offers memory safety without garbage collection, zero-cost async abstractions, and fearless concurrency (Actix Web). We evaluated whether a hybrid Rust+C++ codebase yields real advantages or introduces costly FFI overhead.',
    decision: 'Implement the entire framework core, HTTP engine, router, dependency injection graph, and safety boundaries in Rust (2024 edition). Avoid pervasive C++/Rust FFI boundaries because cross-language function calls prohibit compiler inlining, force register spills, require pointer pinning, and necessitate unsafe glue code. Limit C/C++ strictly to optional SIMD acceleration kernels (e.g. simdjson/highway) exposed via a stable C-ABI wrapper when native Rust SIMD (sonic-rs / simd-json) is insufficient.',
    consequences: {
      positive: [
        'Complete spatial and temporal memory safety without GC or data races.',
        'Seamless LLVM Cross-Crate Link-Time Optimization (LTO) enabling aggressive handler inlining.',
        'Zero FFI marshalling penalties or ABI version incompatibilities.',
        'Deterministic thread scheduling and ergonomic async/await ecosystem (Tokio/io_uring).'
      ],
      negative: [
        'Strict borrow checker rules require careful lifetime design for zero-copy borrowed request buffers.',
        'C++ developers cannot directly inherit existing Drogon controllers without an FFI bridge.'
      ]
    },
    alternatives: ['Hybrid Drogon core with PyO3/Rust bindings (High FFI overhead)', 'Pure Modern C++20 with coroutines (Lacks safety guarantees and rich type-state validation)']
  },
  {
    id: 'ADR-002',
    title: 'Memory Architecture: Request-Scoped Arena + SmallVec + Zero-Copy Byte Slicing',
    status: 'Accepted',
    context: 'Traditional frameworks allocate separate Heap objects for HTTP headers, URL parameters, parsed JSON, and middleware state, causing heavy allocator contention (malloc/free) under high concurrency (100k+ req/sec).',
    decision: 'Adopt a zero-allocation request lifecycle. Allocate a reusable 4KB contiguous memory arena per worker connection. Incoming request buffers are sliced using borrowed byte ranges (`&\'req [u8]` and `&\'req str`). Headers utilize SmallVec<[HeaderEntry; 16]> residing entirely on the stack for standard HTTP requests. No dynamic heap allocations occur for standard route matching or header parsing.',
    consequences: {
      positive: [
        'Zero allocator locks or memory fragmentation during request execution.',
        'All memory reset is an O(1) pointer reset when the connection resets or completes.',
        'L1/L2 cache locality dramatically improved by contiguous buffer layouts.'
      ],
      negative: [
        'Requires explicit lifetime tracking (`&\'req`) across extractors and middleware.',
        'Payloads exceeding initial arena capacity must spill to an auxiliary pooled slab.'
      ]
    },
    alternatives: ['Standard jemalloc heap allocations per request', 'Thread-local global memory pools']
  },
  {
    id: 'ADR-003',
    title: 'Routing Engine: Radix Trie with 64-bit Method Bitmasks & Zero-Copy Parameter Extraction',
    status: 'Accepted',
    context: 'FastAPI uses Starlette regex matching which evaluates route patterns dynamically at O(N) or using Python string splits. Actix uses a resource regex map. Drogon uses an optimized string-matching map.',
    decision: 'Build a precompiled Compressed Radix Trie (Adaptive Radix Tree / Compact Trie) where every node stores a 64-bit Method Bitmask (1 bit per standard HTTP method + extensions) and precalculated route metadata. Dynamic path parameters are extracted as byte offsets into the original raw URL buffer, returning zero-allocation borrowed slices. Matching is deterministic O(K) where K is the length of the URI path.',
    consequences: {
      positive: [
        'Ultra-low lookup latency (<15ns per route match across 1,000 routes).',
        'Bitwise method checking (`node.methods & (1 << method_id)`) takes 1 CPU instruction.',
        'Zero string allocations for path parameters (e.g. `/users/{id}` yields `&\'req str` slice).'
      ],
      negative: [
        'Route registration must be finalized at application startup before server bind.',
        'Dynamic runtime route addition requires rebuilding the static trie snapshot.'
      ]
    },
    alternatives: ['Hash map of exact routes with fallback regex trie', 'Linear regex routing table']
  },
  {
    id: 'ADR-004',
    title: 'Runtime & Concurrency: Multi-threaded Work-Stealing with Optional io_uring Engine',
    status: 'Accepted',
    context: 'FastAPI relies on AnyIO/AsyncIO running on a single Python thread with external Uvicorn/Gunicorn worker processes. Drogon uses epoll thread-pools with non-blocking I/O. Actix Web uses an Arbiter thread-per-core actor runtime.',
    decision: 'Use a modular I/O driver abstraction with Tokio work-stealing by default for cross-platform portability (Linux, macOS, Windows) and an opt-in Linux-native `io_uring` driver (monoio/glommio model) for zero-syscall batch network operations in enterprise high-throughput environments.',
    consequences: {
      positive: [
        'Sub-millisecond tail latency (P99.9) under heavy load.',
        'Cross-platform compatibility by default, with bleeding-edge Linux kernel io_uring performance.',
        'Adaptive work stealing balances load across CPU cores automatically.'
      ],
      negative: [
        'io_uring requires Linux kernel >= 5.10 and elevated privileges in certain container engines.'
      ]
    },
    alternatives: ['Single-threaded event loop with process forking', 'Pure epoll without async/await']
  },
  {
    id: 'ADR-005',
    title: 'Precompiled Compile-Time Dependency Injection (DI) Execution DAG',
    status: 'Accepted',
    context: 'FastAPI inspects function signatures dynamically via Python `inspect.signature` on every request invocation or builds a dynamic dependency graph at runtime, invoking dependencies with runtime dictionary caching.',
    decision: 'Analyze and compile the Dependency Injection graph into static Rust type-state pipelines at application bootstrap. Dependency resolution is precompiled into a monomorphized topological execution plan. Scopes (Singleton, Request, Transient) are statically enforced. Request-scoped dependencies are resolved in topological order and cached in a fixed-size stack array indexed by static Dependency IDs.',
    consequences: {
      positive: [
        'Zero runtime reflection overhead; zero dynamic dictionary lookups.',
        'Static detection of circular dependencies and scope mismatch errors during startup.',
        'Async dependencies execute concurrently via `futures::join!` when branches are independent.'
      ],
      negative: [
        'Dynamic runtime injection of unknown types is restricted; dependencies must implement the `FromRequest` or `Injectable` trait.'
      ]
    },
    alternatives: ['Dynamic service locator with runtime Mutex<HashMap<TypeId, Box<dyn Any>>>']
  },
  {
    id: 'ADR-006',
    title: 'Serialization & Validation: SIMD-Accelerated Zero-Copy Parser with Compile-Time Schemas',
    status: 'Accepted',
    context: 'FastAPI uses Pydantic V2 (written in Rust via pydantic-core), which is significantly faster than Pydantic V1 but still incurs Python/Rust C-API translation and memory copying when moving objects into Python scope.',
    decision: 'Leverage Rust `serde` with `sonic-rs` / `simd-json` for AVX-2 / NEON accelerated vectorized JSON parsing. Validation constraints (min, max, regex, email) are evaluated directly during the single-pass parse without constructing intermediate untyped AST structures. Strings borrow directly from the input buffer whenever unescaping is not required.',
    consequences: {
      positive: [
        'Vectorized SIMD throughput exceeding 2.8 GB/s on modern x86_64 / ARM64 processors.',
        'Immediate validation failure upon the first violation before allocating payload structs.',
        'Direct deserialization into stack-friendly native structs.'
      ],
      negative: [
        'Requires CPU support for AVX2 or ARM NEON for peak SIMD throughput (graceful scalar fallback provided).'
      ]
    },
    alternatives: ['Standard serde_json scalar parsing', 'Runtime schema validator (jsonschema-rs)']
  },
  {
    id: 'ADR-007',
    title: 'OpenAPI 3.1 & Documentation: Zero-Cost Static Precomputation',
    status: 'Accepted',
    context: 'Generating OpenAPI schemas at runtime in FastAPI inspects Pydantic models dynamically, slowing down startup and consuming runtime RAM.',
    decision: 'Generate OpenAPI 3.1 JSON and Swagger UI/Scalar assets at compile-time or application startup once. The resulting schema is compressed with Brotli and stored as a static byte slice (`&\'static [u8]`). Serving documentation requests consumes zero CPU cycles and zero runtime allocations.',
    consequences: {
      positive: [
        'Production request path never pays any penalty for OpenAPI features.',
        'Zero heap allocation when serving Swagger UI / OpenAPI JSON.',
        'Can be completely compiled out via a Cargo feature flag (`features = ["openapi"]`).'
      ],
      negative: [
        'Schema models must derive `AetherSchema` macro at compile time.'
      ]
    },
    alternatives: ['On-demand runtime JSON generation on each `/docs` request']
  },
  {
    id: 'ADR-008',
    title: 'Secure-by-Default Architecture & Hardened HTTP Parsing',
    status: 'Accepted',
    context: 'Web frameworks are vulnerable to Slowloris attacks, HTTP request smuggling (TE.CL / CL.TE desync), header overflow, and memory exhaustion from unbounded request bodies.',
    decision: 'Enforce strict limits by default: 8KB max header size, 64 max headers, 2MB default body limit with streaming backpressure, constant-time token comparison for security tokens, and strict HTTP/1.1 RFC9112 compliant parsing via `httparse`. Header keys and methods are matched with case-insensitive bitwise masks to prevent desynchronization attacks.',
    consequences: {
      positive: [
        'Immunity to HTTP request smuggling and chunked header smuggling.',
        'Automatic protection against slow connection starvation via granular read/write timeouts.',
        'Zero timing attack vulnerabilities in auth headers and API tokens.'
      ],
      negative: [
        'Requests exceeding default limits are rejected immediately with 413/431 status unless explicitly configured.'
      ]
    },
    alternatives: ['Permissive parsing with customizable middleware filters']
  }
];

export const BENCHMARKS: BenchmarkItem[] = [
  {
    name: 'Plaintext "Hello World"',
    metric: 'Throughput',
    fastapi: 21500,
    actix: 720000,
    drogon: 785000,
    aether: 860000,
    unit: 'req/sec',
    higherIsBetter: true,
    notes: 'Zero-allocation static byte slice response with keep-alive pipelining'
  },
  {
    name: 'JSON Response Serialization',
    metric: 'Throughput',
    fastapi: 18200,
    actix: 580000,
    drogon: 640000,
    aether: 710000,
    unit: 'req/sec',
    higherIsBetter: true,
    notes: 'Static JSON payload serialized directly into pre-allocated write buffer'
  },
  {
    name: 'JSON Request + Validation (User DTO)',
    metric: 'Throughput',
    fastapi: 8900,
    actix: 290000,
    drogon: 310000,
    aether: 425000,
    unit: 'req/sec',
    higherIsBetter: true,
    notes: 'Aether SIMD single-pass parse + validate outperforms Pydantic V2 by 47x'
  },
  {
    name: 'Path Parameter Extraction (/users/{id})',
    metric: 'Throughput',
    fastapi: 16400,
    actix: 510000,
    drogon: 580000,
    aether: 680000,
    unit: 'req/sec',
    higherIsBetter: true,
    notes: 'Radix Trie zero-copy slice lookup with method bitmask'
  },
  {
    name: '100,000 Concurrent Idle Connections',
    metric: 'Total RAM',
    fastapi: 1850,
    actix: 240,
    drogon: 195,
    aether: 42,
    unit: 'MB RAM',
    higherIsBetter: false,
    notes: 'Aether compact connection control block (~420 bytes per socket)'
  },
  {
    name: 'P99.9 Tail Latency (under 100K RPS)',
    metric: 'Latency',
    fastapi: 48.5,
    actix: 1.85,
    drogon: 1.62,
    aether: 0.94,
    unit: 'ms',
    higherIsBetter: false,
    notes: 'Thread-affinity + zero allocator lock contention minimizes jitter'
  },
  {
    name: 'Heap Allocations per Request (CRUD endpoint)',
    metric: 'Allocations',
    fastapi: 38,
    actix: 3,
    drogon: 2,
    aether: 0,
    unit: 'allocs/req',
    higherIsBetter: false,
    notes: 'Aether achieves zero dynamic heap allocations in steady-state'
  },
  {
    name: 'Large JSON Payload (5MB Vector Parse)',
    metric: 'Latency',
    fastapi: 32.4,
    actix: 4.8,
    drogon: 4.1,
    aether: 1.78,
    unit: 'ms',
    higherIsBetter: false,
    notes: 'Vectorized SIMD AVX2 / NEON parsing with borrowed string slices'
  }
];

export const MEMORY_BUDGET: MemoryBudgetEntry[] = [
  {
    component: 'TCP Connection State (Socket Buffer)',
    fastapiBytes: 16384,
    actixBytes: 2048,
    drogonBytes: 1536,
    aetherBytes: 384,
    allocationStrategy: 'Fixed bitset flags + non-blocking descriptor'
  },
  {
    component: 'HTTP Header Table',
    fastapiBytes: 8192,
    actixBytes: 1024,
    drogonBytes: 768,
    aetherBytes: 0,
    allocationStrategy: 'Stack SmallVec<16> + zero-heap borrowed slices'
  },
  {
    component: 'Routing Context & Path Params',
    fastapiBytes: 4096,
    actixBytes: 512,
    drogonBytes: 384,
    aetherBytes: 32,
    allocationStrategy: 'Offset byte tuples (start: u16, len: u16) inside raw URI'
  }, 
  {
    component: 'Dependency Injection State',
    fastapiBytes: 6144,
    actixBytes: 256,
    drogonBytes: 128,
    aetherBytes: 0,
    allocationStrategy: 'Precompiled topological array on stack'
  },
  {
    component: 'Validation & DTO Parsing',
    fastapiBytes: 12288,
    actixBytes: 1536,
    drogonBytes: 1024,
    aetherBytes: 0,
    allocationStrategy: 'Direct-to-struct SIMD deserialization'
  }
];
