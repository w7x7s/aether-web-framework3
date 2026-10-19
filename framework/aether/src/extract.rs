//! Typed request extractors.
//!
//! Extractors let an async handler declare what it needs from the request as typed
//! arguments instead of reaching into [`RequestContext`] manually:
//!
//! ```rust,no_run
//! use aether::prelude::*;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct CreateUser {
//!     name: String,
//! }
//!
//! # async fn example(app: &mut AetherApp) {
//! app.post_async("/users/{id}", |Path(id): Path<u32>, Json(body): Json<CreateUser>| async move {
//!     format!("created user {id} named {}", body.name)
//! });
//! # }
//! ```
//!
//! Extraction rules:
//! - Every argument before the last must implement [`FromRequestParts`] (borrows the context):
//!   [`Path`], [`Query`], [`State`].
//! - The last argument implements [`FromRequest`] (consumes the context): [`Json`], [`Form`],
//!   or [`RequestContext`] itself.
//! - Failures produce a typed rejection response (400 for malformed input, 415 for a wrong
//!   `Content-Type`, 500 for missing state) and the handler never runs.
//!
//! Note: sync handlers (`get`, `post`, …) keep the `Fn(RequestContext) -> R` shape; extractor
//! support is available on the `*_async` registration methods.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use serde::de::value::Error as DeError;
use serde::de::{DeserializeOwned, Deserializer, MapAccess, Visitor};

use aether_http::response::Response;

use crate::app::RequestContext;

/// Extractor that borrows from the request context. Used for every handler argument
/// except the last one.
pub trait FromRequestParts: Sized {
    fn from_parts(ctx: &RequestContext) -> Result<Self, Response>;
}

/// Extractor that consumes the request context. Only the last handler argument may
/// implement this trait.
pub trait FromRequest: Sized {
    fn from_request(ctx: RequestContext) -> Result<Self, Response>;
}

/// Path parameters captured by the route, deserialized into `T`.
///
/// - `Path<u32>` / `Path<String>` and other primitives: the route must capture exactly one
///   parameter.
/// - `Path<MyStruct>`: a struct whose fields match the parameter names.
/// - Positional tuples are **not** supported: the underlying parameter map is unordered.
///   Use a struct, or fall back to [`RequestContext::param`]/[`RequestContext::param_parse`].
pub struct Path<T>(pub T);

/// Query string parameters deserialized into `T` (struct or map). Repeated keys are
/// last-value-wins, matching [`RequestContext::queries_decoded`].
pub struct Query<T>(pub T);

/// JSON request body deserialized into `T`. When the request carries a `Content-Type`, it
/// must be `application/json` (or a `+json` media type); a missing `Content-Type` is parsed
/// leniently.
pub struct Json<T>(pub T);

/// `application/x-www-form-urlencoded` request body deserialized into `T` (struct or map).
pub struct Form<T>(pub T);

/// Shared application state registered via [`crate::AetherApp::manage`]. Extracting state
/// that was never managed produces a 500 response.
pub struct State<T>(pub Arc<T>);

fn reject(status: u16, status_text: &'static str, message: impl Into<String>) -> Response {
    let body = serde_json::json!({ "error": message.into() }).to_string();
    Response::json_status(status, status_text, Bytes::from(body))
}

/// Deserialize `T` from a string map. Structs and maps use the entries directly; primitive
/// targets (including through `Option`) require exactly one entry and parse its value.
/// An empty map deserializes into an all-optional struct (or fails for required fields),
/// matching strict framework behavior.
fn from_string_map<T: DeserializeOwned>(map: &HashMap<String, String>) -> Result<T, String> {
    T::deserialize(ParamsDeserializer { map }).map_err(|e| e.to_string())
}

/// `serde::de::value::MapDeserializer` does not support structs, and `StrDeserializer` does
/// not parse numbers out of strings; both gaps break the extractor use cases (`Path<u32>`,
/// `Path<MyStruct>`). These two minimal deserializers close that gap with full primitive
/// parsing.

struct ParamsDeserializer<'de> {
    map: &'de HashMap<String, String>,
}

impl<'de> ParamsDeserializer<'de> {
    fn single_value(&self) -> Result<&'de str, DeError> {
        if self.map.len() == 1 {
            Ok(self.map.values().next().expect("one entry"))
        } else {
            Err(serde::de::Error::invalid_type(
                serde::de::Unexpected::Map,
                &"exactly one parameter (or a struct/map target for named parameters)",
            ))
        }
    }
}

/// Forward a numeric `deserialize_*` to `ValueDeserializer` on the single map entry.
macro_rules! forward_num {
    ($method:ident) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
            ValueDeserializer {
                value: self.single_value()?,
            }
            .$method(visitor)
        }
    };
}

impl<'de> Deserializer<'de> for ParamsDeserializer<'de> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_map(ParamsMapAccess::new(self.map))
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_map(ParamsMapAccess::new(self.map))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_map(ParamsMapAccess::new(self.map))
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        if self.map.is_empty() {
            visitor.visit_unit()
        } else {
            Err(serde::de::Error::invalid_type(
                serde::de::Unexpected::Map,
                &"unit",
            ))
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        ValueDeserializer {
            value: self.single_value()?,
        }
        .deserialize_bool(visitor)
    }

    forward_num!(deserialize_i8);
    forward_num!(deserialize_i16);
    forward_num!(deserialize_i32);
    forward_num!(deserialize_i64);
    forward_num!(deserialize_i128);
    forward_num!(deserialize_u8);
    forward_num!(deserialize_u16);
    forward_num!(deserialize_u32);
    forward_num!(deserialize_u64);
    forward_num!(deserialize_u128);
    forward_num!(deserialize_f32);
    forward_num!(deserialize_f64);

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        ValueDeserializer {
            value: self.single_value()?,
        }
        .deserialize_char(visitor)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        ValueDeserializer {
            value: self.single_value()?,
        }
        .deserialize_str(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom("byte extraction is not supported"))
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom("byte extraction is not supported"))
    }

    fn deserialize_seq<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom(
            "sequence extraction is not supported: named parameters are unordered; use a struct or map",
        ))
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom("enum extraction is not supported"))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }
}

struct ParamsMapAccess<'de> {
    iter: std::collections::hash_map::Iter<'de, String, String>,
    value: Option<&'de str>,
}

impl<'de> ParamsMapAccess<'de> {
    fn new(map: &'de HashMap<String, String>) -> Self {
        Self {
            iter: map.iter(),
            value: None,
        }
    }
}

impl<'de> MapAccess<'de> for ParamsMapAccess<'de> {
    type Error = DeError;

    fn next_key_seed<K: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DeError> {
        match self.iter.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(serde::de::value::StrDeserializer::new(key))
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, DeError> {
        let value = self.value.take().unwrap_or_default();
        seed.deserialize(ValueDeserializer { value })
    }
}

struct ValueDeserializer<'de> {
    value: &'de str,
}

macro_rules! parse_num {
    ($method:ident, $visit:ident, $ty:ty) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
            let parsed: $ty = self
                .value
                .trim()
                .parse()
                .map_err(|_| serde::de::Error::invalid_value(
                    serde::de::Unexpected::Str(self.value),
                    &stringify!($ty),
                ))?;
            visitor.$visit(parsed)
        }
    };
}

impl<'de> Deserializer<'de> for ValueDeserializer<'de> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_borrowed_str(self.value)
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let parsed: bool = self.value.parse().map_err(|_| {
            serde::de::Error::invalid_value(serde::de::Unexpected::Str(self.value), &"a bool")
        })?;
        visitor.visit_bool(parsed)
    }

    parse_num!(deserialize_i8, visit_i8, i8);
    parse_num!(deserialize_i16, visit_i16, i16);
    parse_num!(deserialize_i32, visit_i32, i32);
    parse_num!(deserialize_i64, visit_i64, i64);
    parse_num!(deserialize_i128, visit_i128, i128);
    parse_num!(deserialize_u8, visit_u8, u8);
    parse_num!(deserialize_u16, visit_u16, u16);
    parse_num!(deserialize_u32, visit_u32, u32);
    parse_num!(deserialize_u64, visit_u64, u64);
    parse_num!(deserialize_u128, visit_u128, u128);
    parse_num!(deserialize_f32, visit_f32, f32);
    parse_num!(deserialize_f64, visit_f64, f64);

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let mut chars = self.value.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => visitor.visit_char(c),
            _ => Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(self.value),
                &"a single character",
            )),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_borrowed_str(self.value)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_string(self.value.to_string())
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom("byte extraction is not supported"))
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom("byte extraction is not supported"))
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        if self.value.is_empty() {
            visitor.visit_unit()
        } else {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(self.value),
                &"an empty string",
            ))
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::custom(
            "sequence extraction is not supported; use a struct or map",
        ))
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, DeError> {
        Err(serde::de::Error::invalid_type(
            serde::de::Unexpected::Str(self.value),
            &"a map (wrong extraction target?)",
        ))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_enum(serde::de::value::StrDeserializer::new(self.value))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }
}

impl<T: DeserializeOwned> FromRequestParts for Path<T> {
    fn from_parts(ctx: &RequestContext) -> Result<Self, Response> {
        from_string_map(&ctx.params)
            .map(Path)
            .map_err(|msg| reject(400, "Bad Request", format!("invalid path parameters: {msg}")))
    }
}

impl<T: DeserializeOwned> FromRequestParts for Query<T> {
    fn from_parts(ctx: &RequestContext) -> Result<Self, Response> {
        let map = match ctx.query_str {
            Some(ref q) => {
                let mut map = HashMap::with_capacity(8);
                for pair in q.split('&') {
                    if pair.is_empty() {
                        continue;
                    }
                    let (k, v) = match pair.split_once('=') {
                        Some((k, v)) => (k, v),
                        None => (pair, ""),
                    };
                    map.insert(
                        aether_http::url::form_decode(k),
                        aether_http::url::form_decode(v),
                    );
                }
                map
            }
            None => HashMap::new(),
        };
        from_string_map(&map)
            .map(Query)
            .map_err(|msg| reject(400, "Bad Request", format!("invalid query parameters: {msg}")))
    }
}

impl<T: Send + Sync + 'static> FromRequestParts for State<T> {
    fn from_parts(ctx: &RequestContext) -> Result<Self, Response> {
        ctx.state::<T>()
            .map(State)
            .ok_or_else(|| reject(500, "Internal Server Error", "managed state not found for type"))
    }
}

impl<T: DeserializeOwned> FromRequest for Json<T> {
    fn from_request(ctx: RequestContext) -> Result<Self, Response> {
        if let Some(content_type) = ctx.header("content-type") {
            let mime = content_type.split(';').next().unwrap_or("").trim();
            if !mime.eq_ignore_ascii_case("application/json") && !mime.ends_with("+json") {
                return Err(reject(
                    415,
                    "Unsupported Media Type",
                    format!("expected application/json body, got {mime}"),
                ));
            }
        }
        serde_json::from_slice::<T>(&ctx.body)
            .map(Json)
            .map_err(|err| reject(400, "Bad Request", format!("invalid JSON body: {err}")))
    }
}

impl<T: DeserializeOwned> FromRequest for Form<T> {
    fn from_request(ctx: RequestContext) -> Result<Self, Response> {
        if let Some(content_type) = ctx.header("content-type") {
            let mime = content_type.split(';').next().unwrap_or("").trim();
            if !mime.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
                return Err(reject(
                    415,
                    "Unsupported Media Type",
                    format!("expected application/x-www-form-urlencoded body, got {mime}"),
                ));
            }
        }
        let text = std::str::from_utf8(&ctx.body)
            .map_err(|_| reject(400, "Bad Request", "form body is not valid UTF-8".to_string()))?;
        let mut map = HashMap::with_capacity(8);
        for pair in text.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            map.insert(
                aether_http::url::form_decode(k),
                aether_http::url::form_decode(v),
            );
        }
        from_string_map(&map)
            .map(Form)
            .map_err(|msg| reject(400, "Bad Request", format!("invalid form body: {msg}")))
    }
}

impl FromRequest for RequestContext {
    fn from_request(ctx: RequestContext) -> Result<Self, Response> {
        Ok(ctx)
    }
}

/// Any parts extractor can also serve as the final (context-consuming) argument; it simply
/// borrows the context one last time. This is what lets `|Path(id): Path<u32>| async { .. }`
/// work as a single-argument handler.
impl<T: FromRequestParts> FromRequest for T {
    fn from_request(ctx: RequestContext) -> Result<Self, Response> {
        T::from_parts(&ctx)
    }
}
