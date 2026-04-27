//! HTTP front of the operator bot.
//!
//! Surface:
//!
//! - `GET /healthz` — liveness + config summary for ops verification.
//! - `GET /version` — build identifier so a verifier can correlate
//!   `/healthz` with a specific commit.
//! - `POST /tools/<name>` — operator tool API (bearer-auth, see
//!   `tools::router`). Tools: `claim.create`, `btc.invoice`,
//!   `claim.tick`, `claim.load`, `claim.update`, `node.boot`,
//!   `dd.dispatch_owner_update`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::{Json, Router, http::StatusCode, routing::get};
use serde::Serialize;
use tracing::info;

use crate::btc::{self, BtcWatcher};
use crate::claim::CURRENT_SCHEMA;
use crate::config::Config;
use crate::github;
use crate::tools;

#[derive(Clone)]
struct AppState {
    cfg: Arc<Config>,
}

pub async fn run(cfg: Config) -> Result<()> {
    let port = cfg.port;
    let cfg_arc = Arc::new(cfg);

    let github = Arc::new(github::Client::new(cfg_arc.github_token.clone()));
    let btc: Arc<dyn BtcWatcher> =
        Arc::new(btc::MempoolSpace::with_base_url(&cfg_arc.mempool_base_url));

    let tool_state = tools::State_ {
        cfg: cfg_arc.clone(),
        github,
        btc,
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .with_state(AppState {
            cfg: cfg_arc.clone(),
        })
        .merge(tools::router(tool_state));

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
    ops_repo: String,
    sweep_address_present: bool,
    price_per_24h_sats: u64,
    mempool_base_url: String,
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
            ops_repo: state.cfg.ops_repo.clone(),
            // Don't echo the literal address — operators may treat
            // it as semi-private even though it's on-chain visible.
            sweep_address_present: !state.cfg.sweep_address.is_empty(),
            price_per_24h_sats: state.cfg.price_per_24h_sats,
            mempool_base_url: state.cfg.mempool_base_url.clone(),
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
            ops_repo: "operator/sats-ops-actuator".into(),
            ops_boot_workflow: "boot-agent.yml".into(),
            ops_owner_workflow: "owner-update.yml".into(),
            ops_ref: "main".into(),
            dd_cp_url: "https://app.devopsdefender.com".into(),
            sweep_address: "bc1q-test".into(),
            price_per_24h_sats: 50_000,
            pending_timeout_secs: 10_800,
            github_token: "test-token".into(),
            tool_api_token: "test-tool-token".into(),
            dd_auth_token: None,
            mempool_base_url: "https://mempool.space/api".into(),
            optimistic_bind_grace_secs: 3600,
        }
    }

    fn router(cfg: Config) -> Router {
        Router::new()
            .route("/healthz", get(healthz))
            .route("/version", get(version))
            .with_state(AppState { cfg: Arc::new(cfg) })
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
        assert_eq!(v["ops_repo"], "operator/sats-ops-actuator");
        assert_eq!(v["sweep_address_present"], true);
        assert_eq!(v["price_per_24h_sats"], 50_000);
        assert_eq!(v["mempool_base_url"], "https://mempool.space/api");
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
