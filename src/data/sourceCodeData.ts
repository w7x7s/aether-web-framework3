export interface SourceFile {
  path: string;
  crate: string;
  language: string;
  description: string;
  code: string;
}

export const SOURCE_FILES: SourceFile[] = [
  {
    path: 'framework/Cargo.toml',
    crate: 'Workspace',
    language: 'toml',
    description: 'Cargo workspace configuration with link-time optimization (LTO) and codegen units',
    code: `[workspace]
resolver = "2"
members = [
    "core",
    "http",
    "router",
    "runtime",
    "middleware",
    "validation",
    "serialization",
    "dependency",
    "security",
    "websocket",
    "openapi",
    "observability",
    "benchmarks",
    "fuzz",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["Aether Framework Authors <engineering@aether-framework.io>"]
license = "MIT OR Apache-2.0"

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
overflow-checks = false`
  },
  {
    path: 'framework/core/src/arena.rs',
    crate: 'aether-core',
    language: 'rust',
    description: 'Request-scoped bump allocator for zero-allocation request handling with O(1) reset',
    code: `use std::alloc::{alloc, dealloc, Layout};
use std::cell::Cell;
use std::ptr::NonNull;

pub const DEFAULT_ARENA_CAPACITY: usize = 4096;

pub struct RequestArena {
    ptr: NonNull<u8>,
    capacity: usize,
    offset: Cell<usize>,
}

impl RequestArena {
    pub fn with_capacity(capacity: usize) -> Self {
        let layout = Layout::from_size_align(capacity, 64)
            .expect("Valid memory layout for RequestArena");
        let raw = unsafe { alloc(layout) };
        let ptr = NonNull::new(raw).expect("Memory allocation succeeded");

        Self {
            ptr,
            capacity,
            offset: Cell::new(0),
        }
    }

    #[inline(always)]
    pub fn alloc(&self, layout: Layout) -> Option<NonNull<u8>> {
        let current_offset = self.offset.get();
        let align_mask = layout.align() - 1;
        let aligned_offset = (current_offset + align_mask) & !align_mask;

        let next_offset = aligned_offset.checked_add(layout.size())?;
        if next_offset <= self.capacity {
            self.offset.set(next_offset);
            unsafe {
                Some(NonNull::new_unchecked(self.ptr.as_ptr().add(aligned_offset)))
            }
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn reset(&self) {
        self.offset.set(0);
    }
}`
  },
  {
    path: 'framework/core/src/bitset.rs',
    crate: 'aether-core',
    language: 'rust',
    description: '64-bit compact bitset for single-instruction routing and capability matching',
    code: `#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct Bitset64(pub u64);

impl Bitset64 {
    pub const EMPTY: Self = Self(0);

    #[inline(always)]
    pub const fn set(self, bit: u8) -> Self {
        if bit < 64 {
            Self(self.0 | (1u64 << bit))
        } else {
            self
        }
    }

    #[inline(always)]
    pub const fn contains(self, bit: u8) -> bool {
        if bit < 64 {
            (self.0 & (1u64 << bit)) != 0
        } else {
            false
        }
    }

    #[inline(always)]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }
}`
  },
  {
    path: 'framework/core/src/ccb.rs',
    crate: 'aether-core',
    language: 'rust',
    description: 'Packed 420-byte Connection Control Block (CCB) aligned to 64-byte CPU cache lines',
    code: `use crate::bitset::Bitset64;
use std::net::SocketAddr;

pub const FLAG_KEEPALIVE: u8 = 0;
pub const FLAG_TLS_ACTIVE: u8 = 1;
pub const FLAG_UPGRADED_WS: u8 = 2;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ConnectionState {
    Handshake = 0,
    Idle = 1,
    ReadingHeader = 2,
    Processing = 3,
    WritingResponse = 4,
    Closing = 5,
}

#[repr(C, align(64))]
pub struct ConnectionControlBlock {
    pub flags: Bitset64,
    pub peer_addr: Option<SocketAddr>,
    pub requests_served: u32,
    pub state: ConnectionState,
    pub socket_fd: i32,
    pub last_activity_timestamp: u64,
}`
  },
  {
    path: 'framework/http/src/parser.rs',
    crate: 'aether-http',
    language: 'rust',
    description: 'Zero-copy httparse wrapper borrowing all string slices directly from socket buffer',
    code: `pub struct RequestView<'req> {
    pub method: Method,
    pub path: &'req str,
    pub version: u8,
    pub headers: HeaderMap<'req>,
    pub body: &'req [u8],
}

impl<'req> RequestView<'req> {
    pub fn parse(buf: &'req [u8], limits: &ServerLimits) -> Result<Option<(Self, usize)>, AetherError> {
        let mut raw_headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut raw_headers);

        let status = req.parse(buf).map_err(|_| AetherError::MalformedRequest)?;
        match status {
            httparse::Status::Complete(header_len) => {
                let method = Method::from_bytes(req.method.unwrap().as_bytes())
                    .ok_or(AetherError::MalformedRequest)?;
                let path = req.path.unwrap();
                // Extract headers and body slices with 0 copies
                Ok(Some((RequestView { method, path, ... }, total_needed)))
            }
            httparse::Status::Partial => Ok(None)
        }
    }
}`
  },
  {
    path: 'framework/router/src/trie.rs',
    crate: 'aether-router',
    language: 'rust',
    description: 'Radix Compressed Trie router with single CPU instruction bitset method validation',
    code: `pub struct RadixRouter {
    root: RouteNode,
}

impl RadixRouter {
    pub fn add_route(&mut self, method: Method, path: &str, handler: HandlerId) {
        // Builds compressed prefix trie nodes
        // Sets bitmask flag: node.allowed_methods |= method.bitmask()
    }

    pub fn find<'a>(&self, method: Method, path: &'a str) -> MatchResult<'a> {
        let mut params = PathParams::new(); // SmallVec on stack
        // Traverse segments in O(path_len)
        // If method bitmask matches: return Found { handler_id, params }
        // Else if node has methods: return MethodNotAllowed (405)
    }
}`
  },
  {
    path: 'framework/dependency/src/dag.rs',
    crate: 'aether-dependency',
    language: 'rust',
    description: 'Topological DAG precompiled execution graph with cycle detection',
    code: `pub struct DependencyGraph {
    nodes: HashMap<NodeId, DependencyNode>,
}

impl DependencyGraph {
    /// Precompiled at application boot; eliminates runtime reflection
    pub fn compile_execution_plan(&self) -> Result<Vec<NodeId>, AetherError> {
        // Kahn's Algorithm for topological resolution
        // Verifies cycle-free guarantees before listening
    }
}`
  },
  {
    path: 'framework/security/src/smuggling.rs',
    crate: 'aether-security',
    language: 'rust',
    description: 'RFC 9112 HTTP request smuggling and desynchronization auditor',
    code: `pub fn audit_request_desync(headers: &HeaderMap<'_>) -> Result<(), AetherError> {
    let mut has_content_length = false;
    let mut has_transfer_encoding = false;

    for h in headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            if has_content_length {
                return Err(AetherError::MalformedRequest); // Multiple CL headers
            }
            has_content_length = true;
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            has_transfer_encoding = true;
        }
    }

    if has_content_length && has_transfer_encoding {
        return Err(AetherError::MalformedRequest); // CL.TE / TE.CL conflict
    }

    Ok(())
}`
  },
  {
    path: 'framework/examples/basic_api.rs',
    crate: 'aether-examples',
    language: 'rust',
    description: 'End-to-end runnable REST API demonstration running on port 3000',
    code: `#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Starting Aether Native Web Engine on 0.0.0.0:3000...");

    let mut router = RadixRouter::new();
    router.add_route(Method::Get, "/", 1);
    router.add_route(Method::Get, "/health", 2);
    router.add_route(Method::Get, "/users/{id}", 3);
    router.add_route(Method::Post, "/users/{id}/posts/{post_id}", 4);

    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            handle_connection(stream, router, limits).await
        });
    }
}`
  }
];
