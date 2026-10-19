//! # Zero-Copy Path Parameter Extraction
//!
//! Stores matched path parameters as borrowed slices (`&'req str`) inside a SmallVec
//! on the stack. Exactly 0 heap allocations for routes with up to 4 URL parameters!

use smallvec::SmallVec;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Param<'a> {
    pub key: &'a str,
    pub value: &'a str,
}

pub type PathParams<'a> = SmallVec<[Param<'a>; 4]>;

