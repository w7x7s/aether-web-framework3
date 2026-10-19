use std::time::Instant;

/// High-performance logging middleware recording request timing with microsecond precision.
#[derive(Debug, Clone)]
pub struct LoggerMiddleware {
    pub enabled: bool,
    pub show_timing: bool,
}

impl Default for LoggerMiddleware {
    fn default() -> Self {
        Self {
            // Disabled on hot path by default in high-throughput benchmarks, configurable for dev
            enabled: false,
            show_timing: false,
        }
    }
}

impl LoggerMiddleware {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Record request start
    #[inline(always)]
    pub fn start(&self) -> Instant {
        Instant::now()
    }

    /// Log request outcome with elapsed time when `show_timing` is set (non-blocking when disabled)
    #[inline(always)]
    pub fn log(&self, method: &str, path: &str, status: u16, start: Instant) {
        if !self.enabled {
            return;
        }
        if self.show_timing {
            println!(
                "⚡ [Aether] {} {} -> {} ({}µs)",
                method,
                path,
                status,
                start.elapsed().as_micros()
            );
        } else {
            println!("⚡ [Aether] {} {} -> {}", method, path, status);
        }
    }
}

