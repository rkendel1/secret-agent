//! `agent-core`: the deterministic core of the coding agent.
//!
//! The model — a local LLM or any remote provider — is a pluggable reasoning
//! component. Structure, knowledge and authority live here. This crate never
//! calls a model and has no provider-specific code.
//!
//! - [`appport::knowledge`]: AppPort contracts, capabilities, integrations,
//!   graphs, verification rules and replacement constraints, derived from the
//!   published `@appport/*` packages.
//! - [`observe`]: what an AppPort project on disk contains and where each
//!   capability is implemented.
//! - [`plan`]: from an intent to a bounded change, its acceptance checks and the
//!   minimal model context.
//! - [`conformance`]: schema conformance and deterministic input synthesis.
//! - [`verify`]: evaluation of declared verification methods and the
//!   accept/reject judgement.

#![forbid(unsafe_code)]

pub mod appport;
pub mod conformance;
pub mod observe;
pub mod plan;
pub mod verify;
