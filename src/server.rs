//! HTTP front of the operator bot.
//!
//! v0 surface:
//!
//! - `GET /healthz` — liveness for ops + the dd-agent's deploy
//!   verification step.
//! - `GET /version` — build-time identifier so a third-party verifier
//!   can correlate /health with a specific commit.
//!
//! Tool-API endpoints (claim.create, claim.load, btc.invoice, etc.)
//! land in subsequent PRs. v0 ships the listener + config so the
//! deploy pipeline has a working binary to wire up.

use std::net::SocketAddr;

use anyhow::Result;
use axum::{Json, Router, http::StatusCode, routing::get};
use serde::Serialize;
use tracing::info;

use crate::claim::CURRENT_SCHEMA;
use crate::config::Config;

#[derive(Clone)]
struct AppState {
    cfg: Config,
}

pub async fn run(cfg: Config) -> Result<()> {
    let port = cfg.port;
    let state = AppState { cfg };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!(%addr, "satsforcompute: listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Serialize)]
struct Healthz {
    ok: bool,
    service: &'static str,
    schema: &'static str,
    state_repo: String,
    sweep_address_present: bool,
    price_per_24h_sats: u64,
    pending_timeout_secs: u64,
}

async fn healthz(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> (StatusCode, Json<Healthz>) {
    (
        StatusCode::OK,
        Json(Healthz {
            ok: true,
            service: "satsforcompute",
            schema: CURRENT_SCHEMA,
            state_repo: state.cfg.state_repo.clone(),
            // Don't echo the literal address — operators may treat
            // it as semi-private even though it's on-chain visible.
            // Surfacing a presence flag is enough for ops liveness.
            sweep_address_present: !state.cfg.sweep_address.is_empty(),
            price_per_24h_sats: state.cfg.price_per_24h_sats,
            pending_timeout_secs: state.cfg.pending_timeout_secs,
        }),
    )
}

#[derive(Serialize)]
struct Version {
    pkg: &'static str,
    version: &'static str,
    schema: &'static str,
}

async fn version() -> Json<Version> {
    Json(Version {
        pkg: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        schema: CURRENT_SCHEMA,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_cfg() -> Config {
        Config {
            port: 0,
            state_repo: "operator/sats-ops".into(),
            code_repo: "satsforcompute/satsforcompute".into(),
            dd_cp_url: "https://app.devopsdefender.com".into(),
            sweep_address: "bc1q-test".into(),
            price_per_24h_sats: 50_000,
            pending_timeout_secs: 10_800,
            github_token: "test-token".into(),
        }
    }

    fn router(cfg: Config) -> Router {
        Router::new()
            .route("/healthz", get(healthz))
            .route("/version", get(version))
            .with_state(AppState { cfg })
    }

    #[tokio::test]
    async fn healthz_returns_200_with_summary() {
        let app = router(test_cfg());
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["service"], "satsforcompute");
        assert_eq!(v["schema"], CURRENT_SCHEMA);
        assert_eq!(v["state_repo"], "operator/sats-ops");
        assert_eq!(v["sweep_address_present"], true);
        assert_eq!(v["price_per_24h_sats"], 50_000);
    }

    #[tokio::test]
    async fn version_returns_pkg_metadata() {
        let app = router(test_cfg());
        let resp = app
            .oneshot(Request::get("/version").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["pkg"], env!("CARGO_PKG_NAME"));
        assert_eq!(v["schema"], CURRENT_SCHEMA);
    }
}
