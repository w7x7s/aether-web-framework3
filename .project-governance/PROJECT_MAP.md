# PROJECT MAP - Current Repository Evidence Map
_Generated: 2026-10-18T16:31:29Z by 4.1-clean-en_
_Root Path: C:\Users\hamza\Desktop\nwfm\aether-web-framework_

## 🧰 Detected Stack
- Rust
- JavaScript/TypeScript

## ⚡ Source & Config Files
Total Files Scanned: 77

| Path | Summary | Description |
|---|---|---|
| `package.json` | Node Config | Dependencies, scripts, and package settings |
| `vite.config.ts` | TypeScript | . | UI / logic file |
| `framework/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `src/App.tsx` | TypeScript | src | Exports: App, methodBitmasks |
| `src/main.tsx` | TypeScript | src | UI / logic file |
| `framework/aether/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/benchmarks/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/core/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/dependency/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/examples/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/examples/basic_api.rs` | Rust Code | framework/examples | Key functions: main |
| `framework/examples/full_async_api.rs` | Rust Code | framework/examples | Key functions: main |
| `framework/fuzz/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/http/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/middleware/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/observability/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/openapi/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/router/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/runtime/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/security/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/serialization/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/tests/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/validation/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `framework/websocket/Cargo.toml` | Rust Config | Defines dependencies and package parameters |
| `src/data/architectureData.ts` | TypeScript | src/data | Exports: ADR_LIST, calls |
| `src/data/sourceCodeData.ts` | TypeScript | src/data | Exports: SOURCE_FILES, DEFAULT_ARENA_CAPACITY, EMPTY |
| `framework/aether/src/app.rs` | Rust Code | framework/aether/src | Key functions: into_response, into_response, into_response, into_response |
| `framework/aether/src/config.rs` | Rust Code | framework/aether/src | Key functions: default, from_env |
| `framework/aether/src/lib.rs` | Rust Code | framework/aether/src | Source executable |
| `framework/aether/src/static_dir.rs` | Rust Code | framework/aether/src | Key functions: serve_from_dir |
| `framework/benchmarks/benches/router_bench.rs` | Rust Code | framework/benchmarks/benches | Key functions: bench_radix_router |
| `framework/core/src/arena.rs` | Rust Code | framework/core/src | Key functions: with_capacity, alloc |
| `framework/core/src/bitset.rs` | Rust Code | framework/core/src | Key functions: new, set, clear, contains |
| `framework/core/src/ccb.rs` | Rust Code | framework/core/src | Key functions: new, is_keepalive, is_tls, increment_requests |
| `framework/core/src/diagnostic.rs` | Rust Code | framework/core/src | Key functions: code_str, title |
| `framework/core/src/error.rs` | Rust Code | framework/core/src | Key functions: error_code, to_diagnostic, fmt |
| `framework/core/src/lib.rs` | Rust Code | framework/core/src | Source executable |
| `framework/dependency/src/dag.rs` | Rust Code | framework/dependency/src | Key functions: new, add_node, compile_execution_plan, default |
| `framework/dependency/src/lib.rs` | Rust Code | framework/dependency/src | Source executable |
| `framework/dependency/src/scope.rs` | Rust Code | framework/dependency/src | Source executable |
| `framework/fuzz/fuzz_targets/http_parse_fuzz.rs` | Rust Code | framework/fuzz/fuzz_targets | Key functions: fuzz_target, main |
| `framework/http/src/header.rs` | Rust Code | framework/http/src | Key functions: value_str |
| `framework/http/src/lib.rs` | Rust Code | framework/http/src | Source executable |
| `framework/http/src/limits.rs` | Rust Code | framework/http/src | Key functions: default |
| `framework/http/src/method.rs` | Rust Code | framework/http/src | Key functions: bit, bitmask, from_bytes, as_str |
| `framework/http/src/multipart.rs` | Rust Code | framework/http/src | Key functions: text, header, parse |
| `framework/http/src/parser.rs` | Rust Code | framework/http/src | Key functions: parse, from_head, parse_head |
| `framework/http/src/response.rs` | Rust Code | framework/http/src | Key functions: ok, json, json_status, text |
| `framework/http/src/static_files.rs` | Rust Code | framework/http/src | Key functions: mime_from_extension, generate_etag, etag_matches, safe_resolve_path |
| `framework/http/src/url.rs` | Rust Code | framework/http/src | Key functions: hex_val, percent_decode_cow, percent_decode, form_decode |
| `framework/middleware/src/compression.rs` | Rust Code | framework/middleware/src | Key functions: default, new, default, apply |
| `framework/middleware/src/cors.rs` | Rust Code | framework/middleware/src | Key functions: default, new, apply, apply_for_origin |
| `framework/middleware/src/lib.rs` | Rust Code | framework/middleware/src | Source executable |
| `framework/middleware/src/logger.rs` | Rust Code | framework/middleware/src | Key functions: default, new, enabled, start |
| `framework/middleware/src/security.rs` | Rust Code | framework/middleware/src | Key functions: default, new, apply |
| `framework/observability/src/lib.rs` | Rust Code | framework/observability/src | Key functions: start, elapsed_micros |
| `framework/openapi/src/lib.rs` | Rust Code | framework/openapi/src | Key functions: new, to_json |
| `framework/router/src/lib.rs` | Rust Code | framework/router/src | Source executable |
| `framework/router/src/node.rs` | Rust Code | framework/router/src | Key functions: new, supports_method, get_handler, add_handler |
| `framework/router/src/params.rs` | Rust Code | framework/router/src | Source executable |
| `framework/router/src/trie.rs` | Rust Code | framework/router/src | Key functions: new, bitmask_to_allow_header, add_route |
| `framework/runtime/src/lib.rs` | Rust Code | framework/runtime/src | Source executable |
| `framework/runtime/src/worker.rs` | Rust Code | framework/runtime/src | Key functions: handle_connection |
| `framework/security/src/constant_time.rs` | Rust Code | framework/security/src | Key functions: constant_time_eq, test_constant_time |
| `framework/security/src/jwt.rs` | Rust Code | framework/security/src | Key functions: default, new, verify_signature |
| `framework/security/src/lib.rs` | Rust Code | framework/security/src | Source executable |
| `framework/security/src/smuggling.rs` | Rust Code | framework/security/src | Key functions: audit_request_desync |
| `framework/serialization/src/json.rs` | Rust Code | framework/serialization/src | Key functions: from_slice, to_bytes |
| `framework/serialization/src/lib.rs` | Rust Code | framework/serialization/src | Source executable |
| `framework/tests/src/lib.rs` | Rust Code | framework/tests/src | Key functions: test_error_codes_and_diagnostics, test_server_banner, test_static_mime_and_etag, test_request_arena_zero_copy |
| `framework/tests/tests/http_e2e.rs` | Rust Code | framework/tests/tests | Key functions: start, stop, header, json |
| `framework/validation/src/lib.rs` | Rust Code | framework/validation/src | Source executable |
| `framework/validation/src/simd.rs` | Rust Code | framework/validation/src | Key functions: validate_ascii_fast, validate_email_format |
| `framework/validation/src/validator.rs` | Rust Code | framework/validation/src | Key functions: validate, try_new, as_str, try_new |
| `framework/websocket/src/connection.rs` | Rust Code | framework/websocket/src | Key functions: default, fmt, source, from |
| `framework/websocket/src/handshake.rs` | Rust Code | framework/websocket/src | Key functions: fmt, validate_handshake |
| `framework/websocket/src/lib.rs` | Rust Code | framework/websocket/src | Key functions: parse_frame |

## 📂 Governance Directory Structure
| File | Role |
|---|---|
| requirements.json | Active tasks - deleted upon completion |
| problems.json | Active bugs or obstacles |
| proposals.json | Proposed additions requiring approval |
| criticisms.json | Architecture or code quality concerns |
| PROJECT_MAP.md | Generated repository file index |
| OWNER_VISION.md | Read-only owner intent and constraints |
| SYSTEM_PROMPT.md | Project-independent AI instructions |