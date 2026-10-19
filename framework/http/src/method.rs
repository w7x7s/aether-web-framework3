//! # Fast Bitset-backed HTTP Method Representation
//!
//! Enables checking supported methods in a single CPU instruction:
//! `node.allowed_methods & (1 << method.bit()) != 0`

use std::fmt;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub enum Method {
    Get = 0,
    Post = 1,
    Put = 2,
    Delete = 3,
    Patch = 4,
    Head = 5,
    Options = 6,
    Trace = 7,
    Connect = 8,
    Custom(u8),
}

impl Method {
    #[inline(always)]
    pub const fn bit(self) -> u8 {
        match self {
            Method::Get => 0,
            Method::Post => 1,
            Method::Put => 2,
            Method::Delete => 3,
            Method::Patch => 4,
            Method::Head => 5,
            Method::Options => 6,
            Method::Trace => 7,
            Method::Connect => 8,
            Method::Custom(n) => (n % 55) + 9,
        }
    }

    #[inline(always)]
    pub const fn bitmask(self) -> u64 {
        1u64 << self.bit()
    }

    pub fn from_bytes(slice: &[u8]) -> Option<Self> {
        match slice {
            b"GET" => Some(Method::Get),
            b"POST" => Some(Method::Post),
            b"PUT" => Some(Method::Put),
            b"DELETE" => Some(Method::Delete),
            b"PATCH" => Some(Method::Patch),
            b"HEAD" => Some(Method::Head),
            b"OPTIONS" => Some(Method::Options),
            b"TRACE" => Some(Method::Trace),
            b"CONNECT" => Some(Method::Connect),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
            Method::Connect => "CONNECT",
            Method::Custom(_) => "CUSTOM",
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
