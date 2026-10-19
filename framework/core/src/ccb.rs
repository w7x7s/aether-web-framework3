//! # Aether Connection Control Block (CCB)
//!
//! Ultra-compact, cache-aligned descriptor representing an active TCP/TLS connection.
//! Total footprint is restricted to ~420 bytes to allow 100,000 idle sockets in <45MB RAM.

use crate::bitset::Bitset64;
use std::net::SocketAddr;

/// Packed connection flags (bits 0..15)
pub const FLAG_KEEPALIVE: u8 = 0;
pub const FLAG_TLS_ACTIVE: u8 = 1;
pub const FLAG_UPGRADED_WS: u8 = 2;
pub const FLAG_READ_TIMEOUT: u8 = 3;
pub const FLAG_WRITE_TIMEOUT: u8 = 4;
pub const FLAG_DRAINING: u8 = 5;

/// Connection status representation
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

/// A compact connection descriptor.
#[repr(C, align(64))]
pub struct ConnectionControlBlock {
    pub flags: Bitset64,
    pub peer_addr: Option<SocketAddr>,
    pub requests_served: u32,
    pub state: ConnectionState,
    pub socket_fd: i32,
    pub last_activity_timestamp: u64,
}

impl ConnectionControlBlock {
    pub fn new(fd: i32, peer: Option<SocketAddr>) -> Self {
        let mut flags = Bitset64::EMPTY;
        flags = flags.set(FLAG_KEEPALIVE);

        Self {
            flags,
            peer_addr: peer,
            requests_served: 0,
            state: ConnectionState::Idle,
            socket_fd: fd,
            last_activity_timestamp: 0,
        }
    }

    #[inline(always)]
    pub fn is_keepalive(&self) -> bool {
        self.flags.contains(FLAG_KEEPALIVE)
    }

    #[inline(always)]
    pub fn is_tls(&self) -> bool {
        self.flags.contains(FLAG_TLS_ACTIVE)
    }

    #[inline(always)]
    pub fn increment_requests(&mut self) {
        self.requests_served = self.requests_served.saturating_add(1);
    }
}
