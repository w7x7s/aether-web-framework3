# Aether Production Deployment Guide

## 1. Compiler Optimizations (`Cargo.toml`)
For maximum throughput and minimal tail latency:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
overflow-checks = false
```

Build command:
```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

## 2. Linux Kernel Sysctl Tuning
Apply the following kernel parameters for high concurrency:

```sysctl
# Increase socket backlog & max open files
net.core.somaxconn = 65535
fs.file-max = 2097152

# Epoll / Socket buffer tuning
net.ipv4.tcp_max_syn_backlog = 65535
net.ipv4.tcp_fin_timeout = 15
net.ipv4.tcp_tw_reuse = 1
net.ipv4.ip_local_port_range = 1024 65535

# Fast TCP Keep-Alive
net.ipv4.tcp_keepalive_time = 300
net.ipv4.tcp_keepalive_intvl = 15
net.ipv4.tcp_keepalive_probes = 5
```

## 3. Container Dockerfile
```dockerfile
# Multi-stage build with native binary output
FROM rust:1.80-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY . .
RUN cargo build --release -p aether-examples

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=builder /app/target/release/basic_api /usr/local/bin/aether-server
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/aether-server"]
```
