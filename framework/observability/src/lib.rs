//! # Aether Observability & Low-Overhead Telemetry

use std::time::Instant;

pub struct RequestTimer {
    start: Instant,
}

impl RequestTimer {
    #[inline(always)]
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    #[inline(always)]
    pub fn elapsed_micros(&self) -> u128 {
        self.start.elapsed().as_micros()
    }
}
