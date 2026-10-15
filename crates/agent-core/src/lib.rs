//! `agent-core`: the deterministic core of the coding agent.
//!
//! The model — a local LLM or any remote provider — is a pluggable reasoning
//! component. Structure, knowledge and authority live here. This crate never
//! calls a model and has no provider-specific code.
//!
//! - [`appport::knowledge`]: AppPort contracts, capabilities, integrations,
//!   graphs, verification rules and replacement constraints, derived from the
//!   published `@appport/*` packages.

#![forbid(unsafe_code)]

pub mod appport;
