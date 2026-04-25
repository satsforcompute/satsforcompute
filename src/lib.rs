//! Sats for Compute — pay BTC, get attested DevOpsDefender compute.
//!
//! See `SATS_FOR_COMPUTE_SPEC.md` in the `devopsdefender/dd` repo for
//! the design. This crate is the operator-side bot's tool server +
//! state model. The bot owns:
//!
//! - claim issues on a GitHub repo (canonical state)
//! - BTC invoice generation (BDK in a separate enclave workload)
//! - mempool / 1-conf payment watching
//! - DevOpsDefender VM lifecycle (boot, /owner reassign, reclaim)
//! - workflow_dispatch as the privileged-action actuator
//!
//! v0 scope: scaffold + `Claim` manifest schema (`s12e.claim.v1`).
//! Real handlers are stubs; `/healthz` is the only live route.

pub mod btc;
pub mod claim;
pub mod config;
pub mod github;
pub mod lifecycle;
pub mod server;
pub mod tools;
