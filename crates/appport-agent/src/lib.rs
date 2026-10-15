//! `appport-agent`: the closed loop.
//!
//! observe → plan → (model) → apply → verify → accept, reject or retry.
//!
//! Only one step asks a model anything: writing the replacement implementation.
//! Observation, the integration graph, the plan, the probes and the final
//! accept/reject decision are made by `agent-core`, deterministically, from the
//! contract the application's own runtime publishes and from what the running
//! application answers.

pub mod collect;
pub mod edit;
pub mod host;
pub mod report;
pub mod run;

pub use run::{run, RunOptions};
