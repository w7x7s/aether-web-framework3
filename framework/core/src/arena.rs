//! # Aether Request-Scoped Arena Allocator
//!
//! Provides zero-allocation bump allocation for the lifetime of a single HTTP request.
//! When a request finishes, resetting the arena is an O(1) pointer reset.

use std::alloc::{alloc, dealloc, Layout};
use std::cell::Cell;
use std::ptr::NonNull;

/// Default arena capacity per connection worker: 4KB (fits 99% of requests)
pub const DEFAULT_ARENA_CAPACITY: usize = 4096;

/// Base alignment of the backing allocation. Requests for stricter alignment cannot be
/// satisfied and are refused rather than returned misaligned.
const ARENA_ALIGN: usize = 64;

/// A cache-friendly, bump-pointer memory arena scoped to a single request lifecycle.
///
/// `reset` takes `&mut self` on purpose. With a shared receiver, a caller could hold a slice
/// returned by [`Self::alloc_slice`], reset the arena, allocate again, and receive a second
/// slice over the same bytes — while the first slice was still live and the second allocation
/// had already written through a raw pointer. Requiring exclusive access makes the borrow
/// checker reject that sequence instead of leaving it as latent UB.
pub struct RequestArena {
    ptr: NonNull<u8>,
    capacity: usize,
    offset: Cell<usize>,
}

impl RequestArena {
    /// Creates a new arena with the specified capacity aligned to 64-byte cache lines.
    pub fn with_capacity(capacity: usize) -> Self {
        // `alloc` requires a non-zero size, so the backing block always has at least one
        // byte; `self.capacity` still records what the caller asked for, which is what the
        // bump pointer is checked against.
        let layout = Layout::from_size_align(capacity.max(1), ARENA_ALIGN)
            .expect("Valid memory layout for RequestArena");
        let raw = unsafe { alloc(layout) };
        let ptr = NonNull::new(raw).expect("Memory allocation succeeded");

        Self {
            ptr,
            capacity,
            offset: Cell::new(0),
        }
    }

    /// Allocates `size` bytes with the specified alignment from the arena.
    ///
    /// # Safety
    /// Returned memory is valid for the duration of the request until `reset()` is called.
    #[inline(always)]
    pub fn alloc(&self, layout: Layout) -> Option<NonNull<u8>> {
        // The backing block is only `ARENA_ALIGN`-aligned, so an aligned *offset* is not an
        // aligned *address* for stricter requests.
        if layout.align() > ARENA_ALIGN {
            return None;
        }

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

    /// Allocates and initializes a borrowed slice `&'req [T]` in the arena.
    #[inline(always)]
    pub fn alloc_slice<T: Copy>(&self, slice: &[T]) -> Option<&[T]> {
        if slice.is_empty() {
            return Some(&[]);
        }
        let layout = Layout::for_value(slice);
        let dest = self.alloc(layout)?;
        unsafe {
            std::ptr::copy_nonoverlapping(slice.as_ptr(), dest.as_ptr() as *mut T, slice.len());
            Some(std::slice::from_raw_parts(dest.as_ptr() as *const T, slice.len()))
        }
    }

    /// O(1) Instant reset for the next incoming request on the same persistent connection.
    ///
    /// Takes `&mut self` so no outstanding [`Self::alloc_slice`] borrow can survive it.
    #[inline(always)]
    pub fn reset(&mut self) {
        self.offset.set(0);
    }

    /// Returns current allocated bytes in the arena.
    #[inline(always)]
    pub fn bytes_used(&self) -> usize {
        self.offset.get()
    }

    /// Number of bytes still available without growing.
    #[inline(always)]
    pub fn bytes_remaining(&self) -> usize {
        self.capacity.saturating_sub(self.offset.get())
    }
}

impl Drop for RequestArena {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.capacity.max(1), ARENA_ALIGN).unwrap();
        unsafe {
            dealloc(self.ptr.as_ptr(), layout);
        }
    }
}

// RequestArena exclusively owns its heap allocation and is safe to transfer between threads.
unsafe impl Send for RequestArena {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_bump_and_reset() {
        let mut arena = RequestArena::with_capacity(1024);
        let s = arena.alloc_slice(b"hello aether").expect("Alloc slice");
        assert_eq!(s, b"hello aether");
        assert!(arena.bytes_used() >= 12);

        arena.reset();
        assert_eq!(arena.bytes_used(), 0);
    }

    #[test]
    fn test_arena_respects_alignment() {
        let arena = RequestArena::with_capacity(1024);
        for align in [1usize, 2, 4, 8, 16, 32, 64] {
            let layout = Layout::from_size_align(8, align).unwrap();
            let p = arena.alloc(layout).expect("alloc");
            assert_eq!(p.as_ptr() as usize % align, 0, "align {}", align);
        }
    }

    /// Over-aligned requests cannot be satisfied from a 64-byte-aligned block; refusing is
    /// the only sound answer. Previously they were returned misaligned.
    #[test]
    fn test_arena_refuses_over_alignment() {
        let arena = RequestArena::with_capacity(1024);
        for align in [128usize, 256, 4096] {
            let layout = Layout::from_size_align(8, align).unwrap();
            assert!(arena.alloc(layout).is_none(), "align {} must be refused", align);
        }
    }

    #[test]
    fn test_arena_exhaustion_returns_none() {
        let arena = RequestArena::with_capacity(64);
        assert!(arena.alloc(Layout::from_size_align(64, 1).unwrap()).is_some());
        assert!(arena.alloc(Layout::from_size_align(1, 1).unwrap()).is_none());
    }

    #[test]
    fn test_arena_zero_capacity_does_not_crash() {
        let arena = RequestArena::with_capacity(0);
        assert_eq!(arena.bytes_used(), 0);
        assert!(arena.alloc(Layout::from_size_align(1, 1).unwrap()).is_none());
        drop(arena);
    }

    #[test]
    fn test_arena_empty_slice() {
        let arena = RequestArena::with_capacity(64);
        let s: &[u8] = arena.alloc_slice(&[]).unwrap();
        assert!(s.is_empty());
        assert_eq!(arena.bytes_used(), 0);
    }

    #[test]
    fn test_arena_used_and_remaining_agree() {
        let arena = RequestArena::with_capacity(256);
        let before = arena.bytes_remaining();
        arena.alloc(Layout::from_size_align(100, 1).unwrap()).unwrap();
        assert_eq!(arena.bytes_used() + arena.bytes_remaining(), before);
    }
}
