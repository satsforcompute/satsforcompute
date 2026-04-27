//! Sats for Compute — pay BTC, get attested DevOpsDefender compute.
//!
//! See `SATS_FOR_COMPUTE_SPEC.md` in the `devopsdefender/dd` repo for
//! the design. This crate is the operator-side bot's tool server +
//! claim-manifest schema. It is a pure tool/policy layer — no
//! background loops. State transitions fire from explicit tool calls:
//! external schedulers (cron, the integration test harness, an LLM
//! frontend) drive the bot.

pub mod btc;
pub mod claim;
pub mod config;
pub mod github;
pub mod server;
pub mod tools;
