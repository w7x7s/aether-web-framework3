use aether_core::diagnostic::{Diagnostic, ErrorCode, ServerBanner};
use aether_core::error::AetherError;
use aether_http::limits::ServerLimits;
use aether_http::method::Method;
use aether_http::response::Response;
use aether_http::url::{form_decode, percent_decode, percent_decode_cow};
use aether_middleware::{CorsMiddleware, LoggerMiddleware, SecurityHeadersMiddleware};
use aether_router::trie::{MatchResult, RadixRouter};
use aether_websocket::{HandshakeError, WebSocket};
use bytes::{Bytes, BytesMut};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;

use crate::config::AetherConfig;
use crate::handler::Handler;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Trait converting various types into a unified HTTP Response.
pub trait IntoResponse: Send {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        Response::text(self)
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        let bytes = Bytes::from(self);
        Response::bytes(bytes, "text/plain; charset=utf-8")
    }
}

impl IntoResponse for (u16, &'static str) {
    fn into_response(self) -> Response {
        Response::status(self.0, self.1)
    }
}

impl IntoResponse for (u16, String) {
    fn into_response(self) -> Response {
        let mut res = Response::status(self.0, aether_http::response::canonical_reason(self.0));
        res.headers.push(("Content-Type", "text/plain; charset=utf-8".to_string()));
        res.headers.push(("Content-Length", self.1.len().to_string()));
        res.body = Bytes::from(self.1);
        res
    }
}

impl<T, E> IntoResponse for Result<T, E>
where
    T: IntoResponse,
    E: std::fmt::Display + Send,
{
    fn into_response(self) -> Response {
        match self {
            Ok(val) => val.into_response(),
            Err(err) => {
                // The message is caller-supplied and frequently includes user input (a
                // rejected field, an unknown id). Writing it straight into a JSON template
                // produced malformed bodies for any message containing `"` or a newline, and
                // let the error text break out of the value. serde_json escapes it properly.
                let body = serde_json::json!({
                    "error": err.to_string(),
                    "code": "E5002",
                })
                .to_string();
                let mut res = Response::json(Bytes::from(body));
                res.status = 400;
                res.status_text = "Bad Request";
                res
            }
        }
    }
}

/// حاوية الحالة العامة المشتركة للتطبيق والآمنة للاستخدام عبر الـ Threads المتعددة (Thread-Safe Dependency Container).
/// 
/// تستخدم لتطبيق نمط حقن التبعيات (Dependency Injection) بالاعتماد على `TypeId`،
/// مما يسمح بمشاركة اتصالات قواعد البيانات، المخازن المؤقتة، والخدمات المشتركة بين جميع المعالجات.
#[derive(Default, Clone)]
pub struct AppStateContainer {
    items: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl AppStateContainer {
    /// إنشاء حاوية جديدة فارغة لتسجيل التبعيات والخدمات.
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    /// تسجيل خدمة أو كائن حالة جديد داخل الحاوية مع تغليفه بـ `Arc`.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.items.insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// استرجاع مرجع ذري `Arc<T>` للخدمة المطلوبة بناءً على نوعها في وقت التشغيل بـ $O(1)$.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.items
            .get(&TypeId::of::<T>())
            .and_then(|any| any.clone().downcast::<T>().ok())
    }
}

/// سياق الطلب (`RequestContext`) الممرر لكل معالج مسار (Route Handler).
///
/// يحتوي على جميع بيانات الطلب المنقاة مع ملكية منضبطة تمنع تسريبات الذاكرة (No `Box::leak`).
/// يوفر دوال مساعدة لاستخراج المعلمات، الترويسات، وفك تشفير الـ JSON بسرعة فائقة وبدون عمليات نسخ للذاكرة.
pub struct RequestContext {
    pub raw_uri: String,
    pub path: String,
    pub query_str: Option<String>,
    pub method: Method,
    pub params: HashMap<String, String>,
    /// Header lookup with last-value-wins semantics, kept for compatibility.
    ///
    /// Duplicate fields are all retained in `header_values`; use that method whenever
    /// repeated field lines are meaningful (`Cookie`, `X-Forwarded-For`, `Via`, …).
    pub headers: HashMap<String, String>,
    header_multi: HashMap<String, Vec<String>>,
    pub body: Bytes,
    pub request_id: String,
    state: Arc<AppStateContainer>,
}

impl RequestContext {
    /// استخراج قيمة معلمة مسار ديناميكية (Dynamic Route Parameter) مثل `:id` أو `{id}`
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.as_str())
    }

    /// استخراج معلمة المسار وتحويلها تلقائياً إلى نوع رقمي أو منطقي محدد (مثل `i32`, `u64`, `bool`).
    pub fn param_parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.param(key).and_then(|v| v.parse::<T>().ok())
    }

    /// استخراج قيمة معلمة استعلام (Query Parameter) من الرابط (مثل `?page=2&filter=active`).
    ///
    /// The value is returned exactly as it appeared on the wire, still percent-encoded.
    /// Use [`Self::query_decoded`] when the value contains escapes (any non-ASCII text, a
    /// space, `&`, `=`, …).
    pub fn query(&self, key: &str) -> Option<&str> {
        let q = self.query_str.as_deref()?;
        for pair in q.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == key {
                    return Some(v);
                }
            } else if pair == key {
                return Some("");
            }
        }
        None
    }

    /// استخراج معلمة استعلام مع فك تشفير الـ percent-encoding.
    ///
    /// Decoding follows `application/x-www-form-urlencoded` rules, so `+` is a space and
    /// `%D9%85` is `م`. Returns `None` when the key is absent; an absent value (`?flag`)
    /// yields `Some("")`.
    pub fn query_decoded(&self, key: &str) -> Option<String> {
        let q = self.query_str.as_deref()?;
        for pair in q.split('&') {
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            if percent_decode_cow(k) == key {
                return Some(form_decode(v));
            }
        }
        None
    }

    /// Parse a query parameter into a typed value (e.g. `i32`, `u64`, `bool`, etc.)
    pub fn query_parse<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.query(key).and_then(|v| v.parse::<T>().ok())
    }

    /// Parse a percent-decoded query parameter into a typed value.
    pub fn query_parse_decoded<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        self.query_decoded(key).and_then(|v| v.parse::<T>().ok())
    }

    /// Retrieve all query parameters as a Map
    ///
    /// Keys and values are returned raw, exactly as received. See
    /// [`Self::queries_decoded`] for the decoded form.
    pub fn queries(&self) -> HashMap<&str, &str> {
        let mut map = HashMap::new();
        if let Some(ref q) = self.query_str {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    map.insert(k, v);
                } else if !pair.is_empty() {
                    map.insert(pair, "");
                }
            }
        }
        map
    }

    /// Retrieve all query parameters, percent-decoded.
    pub fn queries_decoded(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Some(ref q) = self.query_str {
            for pair in q.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let (k, v) = match pair.split_once('=') {
                    Some((k, v)) => (k, v),
                    None => (pair, ""),
                };
                map.insert(form_decode(k), form_decode(v));
            }
        }
        map
    }

    /// Parses URL-encoded form data (`application/x-www-form-urlencoded`) from request body into key-value map
    pub fn form_data(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Ok(body_str) = self.body_str() {
            for pair in body_str.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    let decoded_k = form_decode(k);
                    let decoded_v = form_decode(v);
                    map.insert(decoded_k, decoded_v);
                } else if !pair.is_empty() {
                    map.insert(form_decode(pair), String::new());
                }
            }
        }
        map
    }

    /// Retrieve an HTTP header case-insensitively, returning the first field line.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.header_values(name).into_iter().next()
    }

    /// Every field line received for `name`, in wire order.
    ///
    /// RFC 9110 §5.3 treats repeated field lines as equivalent to one comma-joined line
    /// for list-valued fields. A `HashMap<String, String>` cannot represent that, so
    /// `header()` only exposes the first line; this method exposes all of them. Values are
    /// not split on commas, because a comma may legitimately be data (for example inside
    /// a cookie value or a quoted string).
    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.header_multi
            .get(&name.to_ascii_lowercase())
            .map(|values| values.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Parse the body as an RFC 7578 `multipart/form-data` payload.
    ///
    /// The returned parts borrow from [`Self::body`], so no file contents are copied.
    /// Returns [`MultipartError::UnsupportedContentType`] when the request is not a
    /// multipart form. The request body is already bounded by the server's configured
    /// body limit before the handler runs.
    pub fn multipart(&self) -> Result<aether_http::multipart::MultipartForm<'_>, aether_http::multipart::MultipartError> {
        aether_http::multipart::MultipartForm::parse(self.header("content-type").unwrap_or(""), &self.body)
    }

    /// قراءة قيمة ملف تعريف الارتباط (Cookie) بالاسم من ترويسة `Cookie`
    pub fn cookie(&self, name: &str) -> Option<&str> {
        let cookie_hdr = self.header("cookie")?;
        for part in cookie_hdr.split(';') {
            let trimmed = part.trim();
            if let Some((k, v)) = trimmed.split_once('=') {
                if k.trim() == name {
                    return Some(v.trim());
                }
            }
        }
        None
    }

    /// استخراج جسم الطلب (Body) كنص UTF-8 مباشرة بدون أي عمليات نسخ للذاكرة.
    pub fn body_str(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.body)
    }

    /// فك تشفير جسم الطلب من صيغة JSON مباشرة إلى بنية بيانات Rust محددة النوع.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }

    /// استرجاع مرجع الخدمة أو الحالة المشتركة المحقونة بنظام الـ DI عبر نوعها `T`.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.state.get::<T>()
    }

    /// الحصول على المعرّف الفريد لتتبع هذا الطلب (Request Trace ID).
    pub fn request_id(&self) -> &str {
        &self.request_id
    }
}

pub type BoxFutureResponse = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;
pub type AsyncHandlerFn = Arc<dyn Fn(RequestContext) -> BoxFutureResponse + Send + Sync + 'static>;
pub type WsHandlerFn = Arc<
    dyn Fn(RequestContext, WebSocket<TcpStream>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>
        + Send
        + Sync
        + 'static,
>;

/// The continuation passed to user middleware: calling it runs the next middleware in the
/// chain, or the route handler when the chain is exhausted.
pub type Next = Arc<dyn Fn(RequestContext) -> BoxFutureResponse + Send + Sync + 'static>;

/// Type-erased user middleware: receives the request plus the continuation, and produces the
/// response (by calling `next`, by short-circuiting, or both).
pub type MiddlewareEntry = Arc<dyn Fn(RequestContext, Next) -> BoxFutureResponse + Send + Sync + 'static>;

/// Build the dispatch chain for one route: wrap `handler` with `middleware` so that the
/// first middleware in the list runs first on the request path.
fn wrap_with_middleware(handler: AsyncHandlerFn, middleware: &[MiddlewareEntry]) -> AsyncHandlerFn {
    let mut next = handler;
    for mw in middleware.iter().rev() {
        let mw = Arc::clone(mw);
        let inner = next;
        next = Arc::new(move |ctx| {
            let mw = Arc::clone(&mw);
            let inner = Arc::clone(&inner);
            mw(ctx, inner)
        });
    }
    next
}

/// كائن تطبيق Aether الأساسي والمسؤول عن إدارة المسارات، البرمجيات الوسيطة، وحلقة تشغيل الخادم.
pub struct AetherApp {
    router: RadixRouter,
    handlers: HashMap<u32, AsyncHandlerFn>,
    ws_handlers: HashMap<u32, WsHandlerFn>,
    next_id: u32,
    limits: ServerLimits,
    state: AppStateContainer,
    cors: Option<CorsMiddleware>,
    compression: Option<aether_middleware::CompressionMiddleware>,
    security: Option<SecurityHeadersMiddleware>,
    logger: LoggerMiddleware,
    user_middleware: Vec<MiddlewareEntry>,
    config: AetherConfig,
    websocket_limits: aether_websocket::WebSocketLimits,
    scheduling: Option<Arc<crate::scheduling::SchedulingLayer>>,
}

impl Default for AetherApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AetherApp {
    /// Create a new application instance with default configuration
    pub fn new() -> Self {
        Self::with_config(AetherConfig::default())
    }

    /// Create an application instance with custom configuration
    pub fn with_config(config: AetherConfig) -> Self {
        let mut limits = ServerLimits::default();
        limits.max_body_bytes = config.max_body_bytes;
        limits.max_header_bytes = config.max_header_bytes;
        limits.request_timeout = config.max_request_time;
        limits.write_timeout = config.max_response_time;
        limits.keep_alive_timeout = config.keep_alive_timeout;

        Self {
            router: RadixRouter::new(),
            handlers: HashMap::new(),
            ws_handlers: HashMap::new(),
            next_id: 1,
            limits,
            state: AppStateContainer::new(),
            cors: None,
            compression: None,
            security: None,
            logger: LoggerMiddleware::new(),
            user_middleware: Vec::new(),
            config,
            websocket_limits: aether_websocket::WebSocketLimits::default(),
            scheduling: None,
        }
    }

    /// Register user middleware that runs around every route registered **after** this call.
    ///
    /// Middleware runs inside the built-in response pipeline: the request reaches middleware
    /// after routing and before the handler, and the response passes through it before CORS,
    /// security headers, and compression are applied. Multiple middlewares run in
    /// registration order on the request path (first registered runs first).
    ///
    /// A middleware may modify the request before calling `next`, short-circuit by returning
    /// a response without calling `next`, or modify the response after `next` returns:
    ///
    /// ```rust,no_run
    /// # use aether::prelude::*;
    /// # fn example(app: &mut AetherApp) {
    /// app.use_middleware(|req: RequestContext, next: Next| async move {
    ///     let mut res = next(req).await;
    ///     res.headers.push(("X-Processed", "true".to_string()));
    ///     res
    /// });
    /// # }
    /// ```
    pub fn use_middleware<F, Fut, R>(&mut self, middleware: F) -> &mut Self
    where
        F: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + Send + 'static,
    {
        self.user_middleware.push(Arc::new(move |ctx, next| {
            let fut = middleware(ctx, next);
            Box::pin(async move { fut.await.into_response() })
        }));
        self
    }

    /// Register managed application state
    pub fn manage<T: Send + Sync + 'static>(&mut self, state: T) -> &mut Self {
        self.state.insert(state);
        self
    }

    /// Enable CORS middleware
    pub fn enable_cors(&mut self, cors: CorsMiddleware) -> &mut Self {
        self.cors = Some(cors);
        self
    }

    /// Enable Security Headers middleware
    pub fn enable_security_headers(&mut self) -> &mut Self {
        self.security = Some(SecurityHeadersMiddleware::new());
        self
    }

    pub fn enable_compression(&mut self) -> &mut Self {
        self.compression = Some(aether_middleware::CompressionMiddleware::default());
        self
    }

    /// Enable per-request access logging to stdout (disabled by default so benchmarks
    /// and hot paths stay silent). `show_timing` appends the handler latency in µs.
    pub fn enable_request_logging(&mut self, show_timing: bool) -> &mut Self {
        self.logger = LoggerMiddleware {
            enabled: true,
            show_timing,
        };
        self
    }

    /// Register a synchronous or asynchronous GET route
    pub fn get<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Get, path, handler)
    }

    /// Register an asynchronous GET route, optionally with typed extractors.
    ///
    /// The handler runs as an async closure of zero to four arguments. Every argument before
    /// the last must implement `FromRequestParts` (`Path`, `Query`, `State`); the last one
    /// implements `FromRequest` (`Json`, `Form`, `RequestContext`). A handler that takes the
    /// context directly must annotate it: `|req: RequestContext| async { .. }`.
    pub fn get_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Get, path, handler)
    }

    /// Register a synchronous POST route
    pub fn post<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Post, path, handler)
    }

    /// Register an asynchronous POST route. See [`Self::get_async`] for extractor syntax.
    pub fn post_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Post, path, handler)
    }

    /// Register a synchronous PUT route
    pub fn put<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Put, path, handler)
    }

    /// Register an asynchronous PUT route. See [`Self::get_async`] for extractor syntax.
    pub fn put_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Put, path, handler)
    }

    /// Register a synchronous DELETE route
    pub fn delete<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Delete, path, handler)
    }

    /// Register an asynchronous DELETE route. See [`Self::get_async`] for extractor syntax.
    pub fn delete_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Delete, path, handler)
    }

    /// Register a synchronous PATCH route
    pub fn patch<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync(Method::Patch, path, handler)
    }

    /// Register an asynchronous PATCH route. See [`Self::get_async`] for extractor syntax.
    pub fn patch_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async(Method::Patch, path, handler)
    }

    /// Register synchronous handler
    pub fn route_sync<F, R>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        self.route_sync_scoped(method, path, handler, &[])
    }

    fn route_sync_scoped<F, R>(
        &mut self,
        method: Method,
        path: &str,
        handler: F,
        scoped: &[MiddlewareEntry],
    ) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.router.add_route(method, path, id);
        let handler = Arc::new(handler);
        let handler: AsyncHandlerFn = Arc::new(move |ctx| {
            let h = Arc::clone(&handler);
            Box::pin(async move { h(ctx).into_response() })
        });
        let mut chain = Vec::with_capacity(self.user_middleware.len() + scoped.len());
        chain.extend(self.user_middleware.iter().map(Arc::clone));
        chain.extend(scoped.iter().map(Arc::clone));
        self.handlers.insert(id, wrap_with_middleware(handler, &chain));
        self
    }

    /// Register an asynchronous handler with an explicit HTTP method.
    ///
    /// Accepts any handler supported by the [`Handler`] trait: `|req: RequestContext| async`,
    /// `|Path(id): Path<u32>| async`, `|Query(q): Query<T>, Json(b): Json<U>| async`, and so
    /// on. Extraction failures return the extractor's rejection response (400/415/500) and the
    /// handler does not run.
    pub fn route_async<F, Args>(&mut self, method: Method, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        self.route_async_scoped(method, path, handler, &[])
    }

    fn route_async_scoped<F, Args>(
        &mut self,
        method: Method,
        path: &str,
        handler: F,
        scoped: &[MiddlewareEntry],
    ) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.router.add_route(method, path, id);
        let mut chain = Vec::with_capacity(self.user_middleware.len() + scoped.len());
        chain.extend(self.user_middleware.iter().map(Arc::clone));
        chain.extend(scoped.iter().map(Arc::clone));
        self.handlers
            .insert(id, wrap_with_middleware(crate::handler::erase(handler), &chain));
        self
    }

    /// Register a WebSocket endpoint at `path` (RFC 6455).
    ///
    /// The handler runs after a successful upgrade and owns the connection until it returns;
    /// use [`WebSocket::recv`] and the send methods. Only `GET` requests are upgraded. The
    /// handshake is validated by the framework (method, `Upgrade`, `Connection`, version 13,
    /// and a canonical 16-byte key), and failures are answered with 400 or 426 before the
    /// handler is invoked. Generic HTTP middleware does not apply to the upgraded session.
    pub fn ws<F, Fut>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<TcpStream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let id = self.next_id;
        self.next_id += 1;
        self.router.add_route(Method::Get, path, id);
        let handler = Arc::new(handler);
        self.ws_handlers.insert(
            id,
            Arc::new(move |ctx, socket| {
                let h = Arc::clone(&handler);
                Box::pin(async move { h(ctx, socket).await })
            }),
        );
        self
    }

    /// Route Grouping with common prefix
    pub fn group<F>(&mut self, prefix: &str, configure: F) -> &mut Self
    where
        F: FnOnce(&mut RouteGroup),
    {
        let mut group = RouteGroup {
            app: self,
            prefix: prefix.to_string(),
            middleware: Vec::new(),
        };
        configure(&mut group);
        self
    }

    /// Register a static byte asset route directly (e.g. css, js, images, icons)
    pub fn static_asset(&mut self, path: &str, content_type: &'static str, bytes: &'static [u8]) -> &mut Self {
        self.get(path, move |_| Response::bytes(Bytes::from_static(bytes), content_type))
    }

    /// Register a static file from disk to be served directly on a path.
    ///
    /// The file is read with `tokio::fs` on every request so disk latency never blocks a
    /// runtime worker. A missing or unreadable file returns 404.
    pub fn static_file(&mut self, path: &str, file_path: &'static str) -> &mut Self {
        self.get_async(path, move |req: RequestContext| async move {
            let _ = &req;
            let Ok(content) = tokio::fs::read(file_path).await else {
                return Response::status(404, "File Not Found");
            };
            let ext = std::path::Path::new(file_path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let mime = aether_http::static_files::mime_from_extension(ext);
            Response::bytes(Bytes::from(content), mime)
        })
    }

    /// Serve a directory tree under `route_prefix` (e.g. `/assets` -> `./public`).
    ///
    /// Requests are resolved through `safe_resolve_path`, so `..`, encoded traversal,
    /// Windows device names, and alternate data streams cannot escape `root_dir`.
    /// Directory listings are disabled and unknown paths return 404.
    pub fn static_dir(
        &mut self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf>,
    ) -> &mut Self {
        self.static_dir_with_cache(route_prefix, root_dir, 3600)
    }

    /// Same as [`Self::static_dir`] with an explicit `max-age` for `Cache-Control`.
    pub fn static_dir_with_cache(
        &mut self,
        route_prefix: &str,
        root_dir: impl Into<std::path::PathBuf>,
        max_age_secs: u32,
    ) -> &mut Self {
        let root = Arc::new(root_dir.into());
        let prefix = route_prefix.trim_end_matches('/');
        let route = if prefix.is_empty() {
            "/*filepath".to_string()
        } else {
            format!("{prefix}/*filepath")
        };

        self.get_async(&route, move |req: RequestContext| {
            let root = Arc::clone(&root);
            async move {
                let relative = req.param("filepath").unwrap_or("").to_string();
                let if_none_match = req.header("if-none-match").map(str::to_string);
                let if_range = req.header("if-range").map(str::to_string);
                let range = req.header("range").map(str::to_string);
                crate::static_dir::serve_from_dir(
                    &root,
                    &relative,
                    if_none_match.as_deref(),
                    if_range.as_deref(),
                    range.as_deref(),
                    max_age_secs,
                )
                .await
            }
        });
        self
    }

    /// Set resource limits for accepted WebSocket connections, including the inactivity
    /// policy (`WebSocketLimits::read_timeout`, 120s by default). Applies to every route.
    #[must_use]
    pub fn with_websocket_limits(mut self, limits: aether_websocket::WebSocketLimits) -> Self {
        self.websocket_limits = limits;
        self
    }

    /// Start the server, listening on `addr` (for example `127.0.0.1:3000` or `[::1]:3000`).
    pub async fn listen(mut self, addr: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        apply_listen_addr(&mut self.config, addr)?;
        let config = self.config.clone();
        let server = self.bind_with_config(config).await?;
        server.serve().await
    }

    /// Start the server with a full [`AetherConfig`].
    pub async fn listen_with_config(self, config: AetherConfig) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let server = self.bind_with_config(config).await?;
        server.serve().await
    }

    /// Bind the listening socket without accepting connections yet.
    ///
    /// This is the entry point for tests and for applications that need the real bound port
    /// before serving (bind `127.0.0.1:0` and read [`AetherServer::local_addr`]).
    pub async fn bind(mut self, addr: &str) -> Result<AetherServer, Box<dyn std::error::Error + Send + Sync>> {
        apply_listen_addr(&mut self.config, addr)?;
        let config = self.config.clone();
        self.bind_with_config(config).await
    }

    /// Bind the listening socket with an explicit configuration.
    pub async fn bind_with_config(
        mut self,
        config: AetherConfig,
    ) -> Result<AetherServer, Box<dyn std::error::Error + Send + Sync>> {
        self.apply_config(config);
        let addr = self.config.listen_addr();

        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                let diag = Diagnostic::new(ErrorCode::E1001ServerStartupFailed, format!("TCP Bind failed: {}", e))
                    .with_context("Address", &addr)
                    .with_hint("Port is already occupied by another running application or requires elevated privileges.")
                    .with_suggestion(format!("Change port via AETHER_PORT environment variable (e.g. AETHER_PORT={})", self.config.port.saturating_add(1)))
                    .with_suggestion("Kill conflicting process: lsof -ti:3000 | xargs kill -9");
                eprintln!("{}", diag.format_terminal(true));
                return Err(Box::new(AetherError::Io(e)));
            }
        };

        let local_addr = listener.local_addr()?;
        Ok(AetherServer {
            app: self,
            listener,
            local_addr,
        })
    }

    /// Apply a configuration object, keeping the derived parser limits in sync.
    ///
    /// `listen_with_config` previously took a config but kept the limits derived from the
    /// config supplied to `with_config`, so body/header limits could silently disagree with
    /// the advertised configuration.
    fn apply_config(&mut self, config: AetherConfig) {
        self.limits.max_body_bytes = config.max_body_bytes;
        self.limits.max_header_bytes = config.max_header_bytes;
        self.limits.request_timeout = config.max_request_time;
        self.limits.write_timeout = config.max_response_time;
        self.limits.keep_alive_timeout = config.keep_alive_timeout;
        self.config = config;
    }

    /// Enable adaptive tree scheduling: bounded connection admission (503 on
    /// overflow instead of an unbounded blocking wait), per-handler execution
    /// budgets, and the divisible-work driver.
    ///
    /// Opt-in: without this call the serve path is byte-for-byte the default
    /// engine behavior. The connection ceiling is kept in sync with
    /// `ServerLimits::max_connections` regardless of what the resource config says.
    pub fn with_scheduling(&mut self, cfg: aether_scheduler::SchedulingConfig) -> &mut Self {
        self.with_scheduling_resources(cfg, aether_scheduler::ResourceConfig::default())
    }

    /// Enable adaptive scheduling with explicit resource ceilings. See
    /// [`Self::with_scheduling`].
    pub fn with_scheduling_resources(
        &mut self,
        cfg: aether_scheduler::SchedulingConfig,
        rcfg: aether_scheduler::ResourceConfig,
    ) -> &mut Self {
        let rcfg = aether_scheduler::ResourceConfig {
            max_connections: self.limits.max_connections,
            ..rcfg
        };
        self.scheduling = Some(crate::scheduling::SchedulingLayer::new(cfg, rcfg));
        self
    }

    /// The scheduling layer, if [`Self::with_scheduling`] was called. Clone it to
    /// drive divisible work from handlers or tests.
    #[must_use]
    pub fn scheduling(&self) -> Option<Arc<crate::scheduling::SchedulingLayer>> {
        self.scheduling.clone()
    }
}

/// A bound Aether server that has not started accepting connections yet.
pub struct AetherServer {
    app: AetherApp,
    listener: TcpListener,
    local_addr: SocketAddr,
}

impl AetherServer {
    /// The address the kernel actually assigned (useful with port `0` in tests).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Serve until `Ctrl+C` (all platforms) or `SIGTERM` (Unix), then drain in-flight requests.
    pub async fn serve(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.serve_with_shutdown(wait_for_shutdown_signal()).await
    }

    /// Serve until `shutdown` resolves, then drain in-flight requests and release the socket.
    ///
    /// The shutdown future is polled on its own task, so it may be a signal listener, a
    /// channel receiver, or any other `Send` future. Once it resolves, the listener is
    /// dropped immediately (new connections are refused) and existing connections are given
    /// up to `min(keep_alive_timeout, 30s)` to finish.
    pub async fn serve_with_shutdown<F>(
        self,
        shutdown: F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let AetherServer { app, listener, local_addr } = self;
        let routes_count = app.handlers.len() + app.ws_handlers.len();
        let banner = ServerBanner {
            env: if app.config.is_production() { "production" } else { "development" },
            address: format!("http://{}", local_addr),
            routes_count,
            workers: app.config.workers,
        };
        println!("{}", banner.render());

        let router = Arc::new(app.router);
        let handlers = Arc::new(app.handlers);
        let ws_handlers = Arc::new(app.ws_handlers);
        let limits = Arc::new(app.limits);
        let ws_limits = app.websocket_limits;
        let state = Arc::new(app.state);
        let cors = app.cors.map(Arc::new);
        let compression = app.compression.map(Arc::new);
        let security = app.security.map(Arc::new);
        let logger = Arc::new(app.logger);
        let layer = app.scheduling.clone();
        // With the scheduling layer active, connection admission is its job
        // (bounded queue + 503 overflow); the semaphore stays as the plain
        // blocking limiter only on the default path.
        let conn_semaphore = Arc::new(tokio::sync::Semaphore::new(if layer.is_some() {
            1 << 30
        } else {
            limits.max_connections
        }));
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(async move {
            shutdown.await;
            let _ = shutdown_tx.send(true);
        });

        // Track connection tasks so shutdown can actually wait for them. The previous code
        // dropped the semaphore and printed "Graceful shutdown complete" while every spawned
        // request was still running.
        let mut connections: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

        // حلقة استقبال اتصالات الـ TCP مع تقييد عدد الاتصالات المتزامنة (Semaphore Connection Limit)
        loop {
            // Reap finished connection tasks so the JoinSet does not grow without bound.
            while connections.try_join_next().is_some() {}

            if *shutdown_rx.borrow() {
                eprintln!("⚡ Shutdown requested, no longer accepting connections.");
                break;
            }

            let permit = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => break,
                p = conn_semaphore.clone().acquire_owned() => match p {
                    Ok(p) => p,
                    Err(_) => break,
                },
            };

            let (mut stream, _) = tokio::select! {
                biased;
                _ = shutdown_rx.changed() => break,
                r = listener.accept() => match r {
                    Ok(pair) => pair,
                    Err(e) => {
                        eprintln!("Accept error: {}", e);
                        // Resource-exhaustion accept errors (descriptor limits, the
                        // Windows connection-aborted queue) would otherwise turn the
                        // accept loop into a busy spin that steals CPU from every
                        // in-flight handler. A short pause bounds the retry rate;
                        // one-off transient errors still recover on the next pass.
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        continue;
                    }
                },
            };
            let router_clone = Arc::clone(&router);
            let handlers_clone = Arc::clone(&handlers);
            let ws_handlers_clone = Arc::clone(&ws_handlers);
            let limits_clone = Arc::clone(&limits);
            let state_clone = Arc::clone(&state);
            let cors_clone = cors.clone();
            let security_clone = security.clone();
            let compression_clone = compression.clone();
            let logger_clone = Arc::clone(&logger);
            let mut conn_shutdown = shutdown_rx.clone();
            let sched_conn = layer.clone();

            // معالجة كل مقبس (Socket Connection) بشكل غير متزامن داخل Tokio Task مستقلة
            connections.spawn(async move {
                let _permit = permit; // تصريح الـ RAII يتحرر تلقائياً عند إغلاق الاتصال
                // Adaptive scheduling: connection admission with a bounded queue.
                // Overflow/timeout gets a protocol-valid 503 before any state exists.
                let mut conn_guard = if let Some(l) = &sched_conn {
                    let conn_mem = limits_clone.max_header_bytes as u64
                        + limits_clone.max_body_bytes as u64
                        + 12 * 1024;
                    match l.admit(aether_scheduler::NewJob::connection(conn_mem)).await {
                        Ok(g) => Some(g),
                        Err(_) => {
                            let mut r = Response::status(503, "Service Unavailable");
                            r.headers.push(("Retry-After", "1".to_string()));
                            r.headers.push(("Connection", "close".to_string()));
                            let mut rej_buf = BytesMut::new();
                            r.write_http1_to(&mut rej_buf);
                            // Bound the rejection write: a client that opens the socket but
                            // never reads would otherwise pin this task and its concurrency
                            // permit forever, exhausting the connection budget.
                            let _ = tokio::time::timeout(
                                limits_clone.write_timeout,
                                stream.write_all(&rej_buf),
                            )
                            .await;
                            // The client's request bytes are still unread in the receive
                            // buffer here. Closing without a graceful FIN makes Windows
                            // send an RST, which discards the just-written 503 — shutdown
                            // flushes the response and closes cleanly instead.
                            let _ = stream.shutdown().await;
                            return;
                        }
                    }
                } else {
                    None
                };
                let mut conn_buf = BytesMut::with_capacity(8192);
                let mut temp_buf = [0u8; 8192];
                let mut write_buf = BytesMut::with_capacity(4096);
                // Wall-clock start of the request currently being received. The per-read
                // timeout alone lets a client trickle one byte per interval forever; this
                // bounds the whole receive phase with the configured request timeout.
                let mut recv_started: Option<Instant> = None;
                let mut served_any = false;

                loop {
                    // 1. محاولة تحليل أي بيانات مكتملة متبقية في البافر التراكمي (دعم Pipelining و Keep-Alive الفوري)
                    loop {
                        if conn_buf.is_empty() {
                            break;
                        }

                        let parse_result = match aether_http::parser::parse_request(&conn_buf, &limits_clone) {
                            Ok(Some((req, owned_body, consumed))) => {
                                let raw_uri = req.raw_uri.to_string();
                                let path = req.path.to_string();
                                let query_str = req.query_str.map(|s| s.to_string());
                                let method = req.method;
                                let version_minor = req.version_minor;
                                let mut header_map = HashMap::with_capacity(req.headers.len());
                                let mut header_multi: HashMap<String, Vec<String>> =
                                    HashMap::with_capacity(req.headers.len());
                                for h in req.headers.iter() {
                                    if let Ok(val) = std::str::from_utf8(h.value) {
                                        let name = h.name.to_string();
                                        // Repeated field lines share one lowercase key, so
                                        // `Cookie` and `cookie` are correctly merged.
                                        header_multi
                                            .entry(name.to_ascii_lowercase())
                                            .or_default()
                                            .push(val.to_string());
                                        header_map.insert(name, val.to_string());
                                    }
                                }
                                // Chunked bodies are decoded into an owned buffer; framed
                                // bodies are copied out of the reused connection buffer.
                                let body = match owned_body {
                                    Some(decoded) => Bytes::from(decoded),
                                    None => Bytes::copy_from_slice(req.body),
                                };
                                Ok(Some((
                                    raw_uri,
                                    path,
                                    query_str,
                                    method,
                                    version_minor,
                                    header_map,
                                    header_multi,
                                    body,
                                    consumed,
                                )))
                            }
                            Ok(None) => Ok(None),
                            Err(err) => Err(err),
                        };

                        match parse_result {
                            Ok(Some((
                                raw_uri,
                                path_str,
                                query_str,
                                method_enum,
                                version_minor,
                                header_map,
                                header_multi,
                                body_bytes,
                                consumed,
                            ))) => {
                                let start_time = logger_clone.start();
                                let req_id_num = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
                                let req_id_str = format!("req_{:012x}", req_id_num);

                                let accept_encoding = header_value_multi(&header_multi, "accept-encoding");
                                let origin = header_value(&header_map, "origin");
                                let close_requested = connection_has_token(&header_multi, "close");
                                // RFC 9112 §6.1 / RFC 9110 §9.6: HTTP/1.0 connections are
                                // closed after one response unless the client explicitly
                                // asked for keep-alive; HTTP/1.1 persists unless close.
                                let keepalive_requested =
                                    version_minor == 0 && connection_has_token(&header_multi, "keep-alive");
                                let mut close_after_response = close_requested
                                    || (version_minor == 0 && !keepalive_requested);

                                // 2. مطابقة المسار عبر محرك الـ Radix Router في زمن O(K)
                                let mut match_result = router_clone.find(method_enum, &path_str);

                                // RFC 9110 §9.3.2 requires every general-purpose server to
                                // support HEAD. When no HEAD route is registered, serve the GET
                                // handler and drop the payload below, preserving its headers.
                                if method_enum == Method::Head
                                    && !matches!(match_result, MatchResult::Found { .. })
                                {
                                    let get_result = router_clone.find(Method::Get, &path_str);
                                    if matches!(get_result, MatchResult::Found { .. }) {
                                        match_result = get_result;
                                    }
                                }

                                // 3. CORS preflight. Only when the application has not
                                // registered its own OPTIONS route for this path — the previous
                                // blanket 204 made user-registered OPTIONS handlers unreachable.
                                let mut preflight: Option<Response> = None;
                                if method_enum == Method::Options
                                    && !matches!(match_result, MatchResult::Found { .. })
                                {
                                    if let Some(ref cors_mw) = cors_clone {
                                        let requested_method =
                                            header_value(&header_map, "access-control-request-method");
                                        let requested_headers =
                                            header_value(&header_map, "access-control-request-headers");

                                        if origin.is_some() && requested_method.is_some() {
                                            // A preflight only succeeds if the actual request
                                            // would have been routable.
                                            let routable = requested_method
                                                .as_deref()
                                                .and_then(|m| Method::from_bytes(m.as_bytes()))
                                                .map(|m| {
                                                    matches!(
                                                        router_clone.find(m, &path_str),
                                                        MatchResult::Found { .. }
                                                    )
                                                })
                                                .unwrap_or(false);

                                            let mut r = if routable {
                                                Response::status(204, "No Content")
                                            } else {
                                                Response::status(403, "Forbidden")
                                            };
                                            let allowed = routable
                                                && cors_mw.apply_preflight(
                                                    &mut r,
                                                    origin.as_deref(),
                                                    requested_method.as_deref(),
                                                    requested_headers.as_deref(),
                                                );
                                            preflight = Some(if allowed {
                                                r
                                            } else {
                                                Response::status(403, "Forbidden")
                                            });
                                        }
                                    }
                                }

                                let is_preflight = preflight.is_some();
                                let mut response = if let Some(r) = preflight {
                                    r
                                } else {
                                    match match_result {
                                    MatchResult::Found { handler_id, params } => {
                                        if let Some(ws_handler_ref) = ws_handlers_clone.get(&handler_id) {
                                            if method_enum != Method::Get {
                                                let mut res = Response::status(405, "Method Not Allowed");
                                                res.add_header("Allow", "GET");
                                                res
                                            } else {
                                                let mut param_map = HashMap::with_capacity(params.len());
                                                for p in params {
                                                    param_map.insert(p.key.to_string(), percent_decode(p.value));
                                                }
                                                let ctx = RequestContext {
                                                    raw_uri,
                                                    path: path_str.clone(),
                                                    query_str,
                                                    method: method_enum,
                                                    params: param_map,
                                                    headers: header_map,
                                                    header_multi,
                                                    body: body_bytes,
                                                    request_id: req_id_str.clone(),
                                                    state: Arc::clone(&state_clone),
                                                };

                                                let upgrade = ctx.header_values("upgrade").join(", ");
                                                let connection = ctx.header_values("connection").join(", ");
                                                let version = ctx.header("sec-websocket-version").map(str::to_string);
                                                let key = ctx.header("sec-websocket-key").map(str::to_string);

                                                match aether_websocket::validate_handshake(
                                                    "GET",
                                                    if upgrade.is_empty() { None } else { Some(&upgrade) },
                                                    if connection.is_empty() { None } else { Some(&connection) },
                                                    version.as_deref(),
                                                    key.as_deref(),
                                                ) {
                                                    Ok(accept) => {
                                                        let handshake = format!(
                                                            "HTTP/1.1 101 Switching Protocols\r\n\
                                                             Upgrade: websocket\r\n\
                                                             Connection: Upgrade\r\n\
                                                             Sec-WebSocket-Accept: {accept}\r\n\r\n"
                                                        );
                                                        let handshake_write = async {
                                                            stream.write_all(handshake.as_bytes()).await?;
                                                            stream.flush().await
                                                        };
                                                        if tokio::time::timeout(
                                                            limits_clone.write_timeout,
                                                            handshake_write,
                                                        )
                                                        .await
                                                        .map_or(true, |result| result.is_err())
                                                        {
                                                            let _ = stream.shutdown().await;
                                                            return;
                                                        }

                                                        // Anything after the request head is already
                                                        // WebSocket framing (or garbage) and must not
                                                        // be parsed as HTTP.
                                                        let _ = conn_buf.split_to(consumed);
                                                        let initial = conn_buf.to_vec();
                                                        let socket = WebSocket::from_parts(
                                                            stream,
                                                            initial,
                                                            ws_limits,
                                                        );
                                                        logger_clone.log("GET", &path_str, 101, start_time);

                                                        let handler = Arc::clone(ws_handler_ref);
                                                        let mut session = tokio::spawn(handler(ctx, socket));
                                                        tokio::select! {
                                                            biased;
                                                            _ = &mut session => {
                                                                // The WebSocket handler returned on its
                                                                // own terms: the connection finished its
                                                                // negotiated life, so report completion.
                                                                // Without this the guard releases via Drop
                                                                // and the job is tallied as a cancellation,
                                                                // skewing the adaptive baselines.
                                                                if let Some(g) = conn_guard.take() {
                                                                    g.complete();
                                                                }
                                                            }
                                                            _ = conn_shutdown.changed() => {
                                                                // Drain aborts a live session: leave the
                                                                // guard so Drop records the cancellation,
                                                                // mirroring the HTTP close path.
                                                                session.abort();
                                                            }
                                                        }
                                                        return;
                                                    }
                                                    Err(HandshakeError::UnsupportedVersion) => {
                                                        let mut res = Response::status(426, "Upgrade Required");
                                                        res.add_header("Sec-WebSocket-Version", "13");
                                                        res
                                                    }
                                                    Err(_) => Response::status(400, "Bad Request"),
                                                }
                                            }
                                        } else if let Some(handler_ref) = handlers_clone.get(&handler_id) {
                                            let handler = Arc::clone(handler_ref);
                                            let mut param_map = HashMap::with_capacity(params.len());
                                            for p in params {
                                                // Routing stays on the raw path so `%2F`
                                                // cannot change segment boundaries; only the
                                                // captured value is decoded.
                                                param_map.insert(p.key.to_string(), percent_decode(p.value));
                                            }

                                            let ctx = RequestContext {
                                                raw_uri,
                                                path: path_str.clone(),
                                                query_str,
                                                method: method_enum,
                                                params: param_map,
                                                headers: header_map,
                                                header_multi,
                                                body: body_bytes,
                                                request_id: req_id_str.clone(),
                                                state: Arc::clone(&state_clone),
                                            };
                                            // Handler execution admission (opt-in scheduling):
                                            // bounded concurrent handlers, 503 on overflow.
                                            // Handlers are indivisible — never preempted.
                                            let ctx_body_mem = ctx.body.len() as u64 + 16_384;
                                            let admission = match &sched_conn {
                                                Some(l) => l.admit(aether_scheduler::NewJob::handler(ctx_body_mem)).await.map(Some),
                                                None => Ok(None),
                                            };
                                            match admission {
                                                Err(_) => {
                                                    let mut r = Response::status(503, "Service Unavailable");
                                                    r.headers.push(("Retry-After", "1".to_string()));
                                                    r
                                                }
                                                Ok(guard) => {
                                                    let task_handle = tokio::spawn(async move {
                                                        // Success must count as completed so handler
                                                        // runs feed the adaptive baselines; on panic
                                                        // or abort the guard still releases via Drop.
                                                        let result = handler(ctx).await;
                                                        if let Some(g) = guard {
                                                            g.complete();
                                                        }
                                                        result
                                                    });
                                                    let abort_handle = task_handle.abort_handle();
                                                    match tokio::time::timeout(limits_clone.request_timeout, task_handle).await {
                                                        Ok(Ok(r)) => r,
                                                        Ok(Err(_)) => Response::status(500, "Internal Server Error"),
                                                        Err(_) => {
                                                            abort_handle.abort();
                                                            let mut r = Response::status(504, "Gateway Timeout");
                                                            r.headers.push(("Connection", "close".to_string()));
                                                            r
                                                        }
                                                    }
                                                }
                                            }
                                        } else {
                                            Response::status(500, "Internal Server Error")
                                        }
                                    }
                                    MatchResult::MethodNotAllowed { allowed_bitmask } => {
                                        let allow_header = aether_router::trie::RadixRouter::bitmask_to_allow_header(allowed_bitmask);
                                        let mut res = Response::status(405, "Method Not Allowed");
                                        res.add_header("Allow", allow_header);
                                        res
                                    }
                                    MatchResult::NotFound => {
                                        Response::status(404, "Not Found")
                                    }
                                }
                                };

                                response.headers.push(("X-Request-ID", req_id_str));

                                if !is_preflight {
                                    if let Some(ref c) = cors_clone {
                                        c.apply_for_origin(&mut response, origin.as_deref());
                                    }
                                }
                                if let Some(ref s) = security_clone {
                                    s.apply(&mut response);
                                }
                                if let Some(ref comp) = compression_clone {
                                    comp.apply(&mut response, accept_encoding.as_deref());
                                }
                                // A handler or framework error can advertise `Connection: close`
                                // itself (e.g. the 504 that aborts a timed-out handler). The close
                                // decision was computed from the *request* alone, so without this
                                // the server would emit `Connection: close` and then keep reading on
                                // a socket it just told the client to abandon — a framing split.
                                if !close_after_response && response.headers.iter().any(|(k, v)| {
                                    k.eq_ignore_ascii_case("connection")
                                        && v.split(',')
                                            .map(|t| t.trim_matches(|c| c == ' ' || c == '\t'))
                                            .any(|t| t.eq_ignore_ascii_case("close"))
                                }) {
                                    close_after_response = true;
                                }
                                if !response.headers.iter().any(|(k, _)| {
                                    k.eq_ignore_ascii_case("connection")
                                }) {
                                    if close_after_response {
                                        response.add_header("Connection", "close".to_string());
                                    } else if version_minor == 0 {
                                        // A persisted HTTP/1.0 connection must advertise it.
                                        response.add_header("Connection", "keep-alive".to_string());
                                    }
                                }

                                if logger_clone.enabled {
                                    logger_clone.log(
                                        &format!("{:?}", method_enum),
                                        &path_str,
                                        response.status,
                                        start_time,
                                    );
                                }

                                // HEAD carries the GET headers (including Content-Length) but
                                // no payload, whether the route was explicit or fell back.
                                write_buf.clear();
                                response.write_http1_to_with_body(&mut write_buf, method_enum != Method::Head);
                                let write_future = async {
                                    stream.write_all(&write_buf).await?;
                                    stream.flush().await
                                };
                                // A mid-write IO error (client gone) must end the
                                // keep-alive loop; continuing would silently swallow
                                // later pipelined responses on a dead socket.
                                if tokio::time::timeout(limits_clone.write_timeout, write_future)
                                    .await
                                    .map_or(true, |result| result.is_err())
                                {
                                    return;
                                }

                                // Advance the buffer by the exact number of consumed bytes
                                let _ = conn_buf.split_to(consumed);
                                served_any = true;
                                // A pipelined head already sitting in the buffer starts its
                                // own deadline now; otherwise the next request starts on read.
                                recv_started = if conn_buf.is_empty() {
                                    None
                                } else {
                                    Some(Instant::now())
                                };

                                if close_after_response {
                                    // The connection finished its negotiated life; count
                                    // it complete before leaving the task.
                                    if let Some(g) = conn_guard.take() {
                                        g.complete();
                                    }
                                    // Graceful FIN: pipelined bytes may still sit unread
                                    // in the receive buffer, and a bare close would make
                                    // Windows RST and discard the response just written.
                                    let _ = stream.shutdown().await;
                                    return;
                                }
                            }
                            Ok(None) => {
                                // Need more data from the network
                                if recv_started.is_some_and(|t| t.elapsed() > limits_clone.request_timeout) {
                                    let mut err_res = Response::status(408, "Request Timeout");
                                    err_res.headers.push(("Connection", "close".to_string()));
                                    write_buf.clear();
                                    err_res.write_http1_to(&mut write_buf);
                                    let _ = tokio::time::timeout(
                                        limits_clone.write_timeout,
                                        stream.write_all(&write_buf),
                                    )
                                    .await;
                                    let _ = stream.shutdown().await;
                                    return;
                                }
                                break;
                            }
                            Err(err) => {
                                let mut err_res = match err {
                                    AetherError::HeaderTooLarge => Response::status(431, "Request Header Fields Too Large"),
                                    AetherError::BodyLimitExceeded => Response::status(413, "Payload Too Large"),
                                    _ => Response::status(400, "Bad Request"),
                                };
                                err_res.headers.push(("Connection", "close".to_string()));
                                write_buf.clear();
                                err_res.write_http1_to(&mut write_buf);
                                // Bound the error write (a stalled client must not pin
                                // the task/permit) and close with FIN so Windows does
                                // not RST away the response over unread request bytes.
                                let _ = tokio::time::timeout(
                                    limits_clone.write_timeout,
                                    stream.write_all(&write_buf),
                                )
                                .await;
                                let _ = stream.shutdown().await;
                                return;
                            }
                        }
                    }

                    // A keep-alive socket must not pin the drain: once shutdown is requested,
                    // the loop exits after the response currently in flight is written.
                    if *conn_shutdown.borrow() {
                        break;
                    }

                    // Read next TCP chunk. While a request is half-received (or before the
                    // first response is served) the strict read timeout bounds slowloris
                    // trickling; once the socket is simply idle between keep-alive requests
                    // the configured keep_alive_timeout applies instead.
                    let read_limit = if recv_started.is_some() || !served_any {
                        limits_clone.read_timeout
                    } else {
                        limits_clone.keep_alive_timeout
                    };
                    let read_result = tokio::select! {
                        biased;
                        _ = conn_shutdown.changed() => break,
                        r = tokio::time::timeout(
                            read_limit,
                            stream.read(&mut temp_buf),
                        ) => r,
                    };

                    match read_result {
                        Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break, // Client closed or read timed out (Slowloris protection)
                        Ok(Ok(n)) => {
                            // Enforce total accumulated buffer limit to prevent memory exhaustion DoS
                            if conn_buf.len() + n > limits_clone.max_header_bytes + limits_clone.max_body_bytes {
                                let mut err_res = Response::status(413, "Payload Too Large");
                                err_res.headers.push(("Connection", "close".to_string()));
                                write_buf.clear();
                                err_res.write_http1_to(&mut write_buf);
                                let _ = tokio::time::timeout(
                                    limits_clone.write_timeout,
                                    stream.write_all(&write_buf),
                                )
                                .await;
                                break;
                            }
                            if recv_started.is_none() {
                                recv_started = Some(Instant::now());
                            }
                            conn_buf.extend_from_slice(&temp_buf[..n]);
                        }
                    }
                }

                let _ = stream.shutdown().await;
                // The connection ended on its own terms (EOF, idle timeout, or clean
                // break): report completion so the job is not tallied as a cancellation.
                // Aborts and panics never reach this line and still release via Drop.
                if let Some(g) = conn_guard {
                    g.complete();
                }
            });
        }

        // Graceful shutdown: stop accepting, finish in-flight requests, then abort stragglers.
        if connections.is_empty() {
            eprintln!("⚡ No active connections; graceful shutdown complete.");
        } else {
            eprintln!("⚡ Draining {} active connection(s)...", connections.len());
            let drain_timeout = limits.keep_alive_timeout.min(Duration::from_secs(30));
            let drained = tokio::time::timeout(drain_timeout, async {
                while connections.join_next().await.is_some() {}
            })
            .await;
            if drained.is_err() {
                eprintln!(
                    "⚡ Graceful shutdown timed out after {:?}; aborting remaining connections.",
                    drain_timeout
                );
                connections.shutdown().await;
            }
            eprintln!("⚡ Graceful shutdown complete.");
        }

        Ok(())
    }
}

/// Helper struct for Route Grouping
pub struct RouteGroup<'a> {
    app: &'a mut AetherApp,
    prefix: String,
    middleware: Vec<MiddlewareEntry>,
}

impl<'a> RouteGroup<'a> {
    /// Register middleware that runs around every route registered **inside this group after
    /// this call**, in addition to the app-wide middleware stack. WebSocket routes do not
    /// participate in middleware.
    pub fn use_middleware<F, Fut, R>(&mut self, middleware: F) -> &mut Self
    where
        F: Fn(RequestContext, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse + Send + 'static,
    {
        self.middleware.push(Arc::new(move |ctx, next| {
            let fut = middleware(ctx, next);
            Box::pin(async move { fut.await.into_response() })
        }));
        self
    }

    pub fn get<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_sync_scoped(Method::Get, &full_path, handler, &self.middleware);
        self
    }

    pub fn get_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_async_scoped(Method::Get, &full_path, handler, &self.middleware);
        self
    }

    pub fn post<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_sync_scoped(Method::Post, &full_path, handler, &self.middleware);
        self
    }

    pub fn post_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_async_scoped(Method::Post, &full_path, handler, &self.middleware);
        self
    }

    pub fn put<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_sync_scoped(Method::Put, &full_path, handler, &self.middleware);
        self
    }

    pub fn put_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_async_scoped(Method::Put, &full_path, handler, &self.middleware);
        self
    }

    pub fn delete<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_sync_scoped(Method::Delete, &full_path, handler, &self.middleware);
        self
    }

    pub fn delete_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_async_scoped(Method::Delete, &full_path, handler, &self.middleware);
        self
    }

    pub fn patch<F, R>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext) -> R + Send + Sync + 'static,
        R: IntoResponse + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_sync_scoped(Method::Patch, &full_path, handler, &self.middleware);
        self
    }

    pub fn patch_async<F, Args>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Handler<Args>,
        Args: 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.route_async_scoped(Method::Patch, &full_path, handler, &self.middleware);
        self
    }

    /// Register a WebSocket endpoint under the group prefix. See [`AetherApp::ws`].
    pub fn ws<F, Fut>(&mut self, path: &str, handler: F) -> &mut Self
    where
        F: Fn(RequestContext, WebSocket<TcpStream>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let full_path = format!("{}{}", self.prefix, path);
        self.app.ws(&full_path, handler);
        self
    }
}

/// Case-insensitive header lookup over the per-request map.
fn header_value(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

/// Combine repeated field lines for a list-valued header (RFC 9110 §5.3).
fn header_value_multi(headers: &HashMap<String, Vec<String>>, name: &str) -> Option<String> {
    headers
        .get(&name.to_ascii_lowercase())
        .map(|values| values.join(", "))
}

/// True when the comma-separated `Connection` header lists `token` (RFC 9110 §5.3, §7.3.1).
/// The whole field value must NOT be compared atomically: clients legitimately send
/// `Connection: keep-alive, close` or `Connection: close, TE`, and an atom check would
/// silently miss the `close` directive, wrongly keeping the socket alive. The token match is
/// case-insensitive with surrounding OWS trimmed, exactly as the ABNF `connection = #token`
/// grammar requires.
fn connection_has_token(
    headers_multi: &HashMap<String, Vec<String>>,
    token: &str,
) -> bool {
    let Some(values) = headers_multi.get("connection") else {
        return false;
    };
    values.iter().any(|v| {
        v.split(',')
            .map(|t| t.trim_matches(|c| c == ' ' || c == '\t'))
            .any(|t| t.eq_ignore_ascii_case(token))
    })
}

/// Parse a `listen` address into the configuration's host and port.
///
/// Splitting on the first `:` broke every IPv6 literal (`[::1]:3000` became host `[` and
/// port ``). Parse the bracketed form explicitly and otherwise take the last colon, which
/// is the port in both `host:port` and bare `::1:3000`.
///
/// A non-empty but unparseable port is a configuration mistake and must surface, not
/// silently fall back to the default port — the server would otherwise start listening
/// somewhere the caller never asked for.
fn apply_listen_addr(config: &mut AetherConfig, addr: &str) -> Result<(), String> {
    let (host, port) = if let Some(rest) = addr.strip_prefix('[') {
        match rest.split_once("]:") {
            Some((h, p)) => (Some(h), p),
            None => (Some(rest.trim_end_matches(']')), ""),
        }
    } else {
        match addr.rsplit_once(':') {
            Some((h, p)) => (Some(h), p),
            // A colon-less address is all digits only in the conventional bare-port
            // form (`"3000"`); otherwise it is a host and the port stays default.
            None if !addr.is_empty() && addr.bytes().all(|b| b.is_ascii_digit()) => (None, addr),
            None => (Some(addr), ""),
        }
    };

    if let Some(h) = host {
        if !h.is_empty() {
            config.host = h.to_string();
        }
    }
    if !port.is_empty() {
        let p: u16 = port.parse().map_err(|_| {
            format!(
                "Aether: invalid port '{port}' in listen address '{addr}'; expected a number in 0..=65535"
            )
        })?;
        config.port = p;
    }
    Ok(())
}

/// Wait for the platform shutdown signal: `Ctrl+C` everywhere, plus `SIGTERM` on Unix.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(sig) => sig,
            Err(e) => {
                eprintln!("Failed to install SIGTERM handler: {}", e);
                match signal::ctrl_c().await {
                    Ok(_) => eprintln!("\n⚡ Received Ctrl+C, initiating graceful shutdown..."),
                    Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
                }
                return;
            }
        };

        tokio::select! {
            result = signal::ctrl_c() => {
                match result {
                    Ok(_) => eprintln!("\n⚡ Received Ctrl+C, initiating graceful shutdown..."),
                    Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
                }
            }
            _ = sigterm.recv() => {
                eprintln!("\n⚡ Received SIGTERM, initiating graceful shutdown...");
            }
        }
    }

    #[cfg(not(unix))]
    {
        match signal::ctrl_c().await {
            Ok(_) => eprintln!("\n⚡ Received Ctrl+C, initiating graceful shutdown..."),
            Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
        }
    }
}
