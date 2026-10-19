use aether::prelude::*;
use aether::{PolicyKind, SchedulingConfig, SchedulingResourceConfig, SliceProgress};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Adaptive scheduling demo:
/// - HTTP traffic runs through the AdaptiveTree admission scheduler (2 handler slots).
/// - A long divisible background job is sliced by `run_slices` and must never
///   starve the web traffic.
/// - `GET /sched-stats` exposes the scheduler's live metrics.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut app = AetherApp::new();

    let scfg = SchedulingConfig {
        policy: PolicyKind::AdaptiveTree,
        ..SchedulingConfig::default()
    };
    let rcfg = SchedulingResourceConfig {
        max_active_handlers: Some(2),
        min_slots: 2,
        ..SchedulingResourceConfig::default()
    };
    app.with_scheduling_resources(scfg, rcfg);
    let layer = app.scheduling().expect("scheduling enabled");

    app.get("/ping", |_req| "pong");

    {
        let layer = Arc::clone(&layer);
        app.get_async("/sched-stats", move |_req: RequestContext| {
            let layer = Arc::clone(&layer);
            async move {
                let m = layer.metrics();
                let r = layer.resources();
                let body = serde_json::json!({
                    "policy": layer.policy_name(),
                    "baseline_work": layer.baseline(),
                    "admitted": m.admitted,
                    "completed": m.completed,
                    "cancelled": m.cancelled,
                    "rejected_queue_full": m.rejected_queue_full,
                    "starvation_promotions": m.starvation_promotions,
                    "grants_issued": m.grants_issued,
                    "work_executed": m.work_executed,
                    "peak_queue": m.peak_queue,
                    "active_handlers": r.active_handlers,
                    "active_connections": r.active_connections,
                    "active_divisible": r.active_divisible,
                    "handler_slots": r.handler_slots,
                    "registered_memory_bytes": r.registered_memory_bytes,
                })
                .to_string();
                Response::json(bytes::Bytes::from(body))
            }
        });
    }

    // A 20 000-unit background job. The scheduler grants it baseline-sized
    // slices, so /ping keeps responding while it grinds through the work.
    let driver_layer = Arc::clone(&layer);
    let executed = Arc::new(AtomicU64::new(0));
    let executed_counter = Arc::clone(&executed);
    tokio::spawn(async move {
        let total: u64 = 20_000;
        let result = driver_layer
            .run_slices(Some(total), 0, move |quota| {
                let executed = Arc::clone(&executed_counter);
                async move {
                    let done_before = executed.load(Ordering::SeqCst);
                    let work = quota.min(total - done_before);
                    // Simulated chunk of background work.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let done = done_before + work;
                    executed.store(done, Ordering::SeqCst);
                    if done >= total {
                        SliceProgress::finished(work)
                    } else {
                        SliceProgress::progressed(work)
                    }
                }
            })
            .await;
        println!("background job finished: {result:?}");
    });

    println!("⚡ Scheduling demo on http://127.0.0.1:3000 (try /ping, /sched-stats)");
    app.listen("127.0.0.1:3000").await
}
