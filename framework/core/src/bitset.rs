//! # Aether Bitset Utilities
//!
//! Compact, allocation-free bit arrays and bitflags for routing method matching,
//! connection flags, and capability tags.

use std::fmt;

/// A 64-bit compact bitset for ultra-fast bitwise masking in a single CPU instruction.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct Bitset64(pub u64);

impl Bitset64 {
    pub const EMPTY: Self = Self(0);
    pub const ALL: Self = Self(u64::MAX);

    #[inline(always)]
    pub const fn new(val: u64) -> Self {
        Self(val)
    }

    #[inline(always)]
    pub const fn set(self, bit: u8) -> Self {
        if bit < 64 {
            Self(self.0 | (1u64 << bit))
        } else {
            self
        }
    }

    #[inline(always)]
    pub const fn clear(self, bit: u8) -> Self {
        if bit < 64 {
            Self(self.0 & !(1u64 << bit))
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

    #[inline(always)]
    pub const fn count_ones(self) -> u32 {
        self.0.count_ones()
    }
}

impl fmt::Debug for Bitset64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bitset64({:#018x}b)", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitset_operations() {
        let mut b = Bitset64::EMPTY;
        b = b.set(0); // GET
        b = b.set(1); // POST
        assert!(b.contains(0));
        assert!(b.contains(1));
        assert!(!b.contains(2));
        assert_eq!(b.count_ones(), 2);

        b = b.clear(0);
        assert!(!b.contains(0));
        assert!(b.contains(1));
    }
}
