mod btcpay;
mod config;
mod db;
mod gcp;
mod lifecycle;
mod local;
mod models;
mod pool;
mod provision;
mod routes;
mod templates;

use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

use crate::config::Config;
use crate::routes::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();

    // Database
    let database = db::open(&config.db_path);
    db::migrate(&database);
    db::seed_plans(&database);

    tracing::info!(port = config.port, db = %config.db_path, "starting dd-market");

    let state = AppState {
        db: database.clone(),
        config: config.clone(),
        btcpay: Arc::new(btcpay::BtcPayClient::new(&config)),
    };

    // Background tasks
    let prov_db = database.clone();
    let prov_config = config.clone();
    tokio::spawn(provision::provisioner_loop(prov_db, prov_config));

    let life_db = database.clone();
    let life_config = config.clone();
    tokio::spawn(lifecycle::lifecycle_loop(life_db, life_config));

    let pool_db = database.clone();
    let pool_config = config.clone();
    tokio::spawn(pool::pool_loop(pool_db, pool_config));

    let app = Router::new()
        .route("/", get(routes::landing))
        .route("/order", post(routes::create_order))
        .route("/order/{id}", get(routes::order_status))
        .route("/order/{id}/status", get(routes::order_status_json))
        .route("/webhooks/btcpay", post(routes::btcpay_webhook))
        .route("/health", get(routes::health))
        .route("/admin/orders", get(routes::admin_orders))
        .route("/admin/nodes", get(routes::admin_nodes))
        .route("/admin/pool", get(routes::admin_pool))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!(addr = %addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("shutting down");
}
