//! # aether-scheduler
//!
//! A protocol-independent adaptive scheduling engine: admission control, bounded
//! queues, per-class work accounting, and pluggable scheduling policies
//! (FIFO, Round Robin, Deficit round robin, Adaptive Tree).
//!
//! The crate has no dependency on Tokio, HTTP, or any Aether protocol code. Every
//! policy call takes an explicit `now: Duration`, so the whole engine can be driven
//! by a deterministic virtual clock in tests and simulators. See
//! `framework/docs/ADAPTIVE_TREE_SCHEDULING_DESIGN.md` for the algorithm contract;
//! the types here mirror that document one-to-one.
//!
//! Work is measured in abstract [`Work`] units reported by the driver. The engine
//! never assumes a unit of work costs a fixed amount of time: rate estimates are
//! learned per job from measured `(work, elapsed)` slice reports.

mod baseline;
mod config;
mod engine;
mod job;
mod policy;
mod quota;
mod resources;
mod sim;
mod tree;

pub use baseline::Baseline;
pub use config::{PolicyKind, ResourceConfig, SchedulingConfig};
pub use engine::{Decision, DecisionKind, Grant, RejectReason, RoundOutcome, SchedulerEngine, SchedulerMetrics};
pub use job::{JobClass, JobId, JobKind, JobRecord, JobState, NewJob, Work};
pub use policy::{AdaptiveTree, DeficitRoundRobin, Fifo, RoundRobin, SchedulingPolicy};
pub use resources::{ResourceSnapshot, Resources};
pub use sim::{JobSpec, LARGE_WORK, Scenario, SimResult, percentile, run, workload};
