//! # Zero-Copy and Compact HTTP Headers
//!
//! Employs `smallvec` to store up to 16 common headers inline on the stack,
//! completely avoiding heap allocations for standard API requests.

use smallvec::SmallVec;

/// A borrowed HTTP Header pair directly pointing into the connection read buffer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HeaderView<'a> {
    pub name: &'a str,
    pub value: &'a [u8],
}

impl<'a> HeaderView<'a> {
    #[inline(always)]
    pub fn value_str(&self) -> Result<&'a str, std::str::Utf8Error> {
        std::str::from_utf8(self.value)
    }
}

/// Stack-allocated header container for up to 16 headers.
pub type HeaderMap<'a> = SmallVec<[HeaderView<'a>; 16]>;

/// Common standard HTTP header token values interned into static strings.
pub mod standard_headers {
    pub const CONTENT_TYPE: &str = "content-type";
    pub const CONTENT_LENGTH: &str = "content-length";
    pub const AUTHORIZATION: &str = "authorization";
    pub const HOST: &str = "host";
    pub const CONNECTION: &str = "connection";
    pub const USER_AGENT: &str = "user-agent";
    pub const ACCEPT: &str = "accept";
    pub const ACCEPT_ENCODING: &str = "accept-encoding";
}
