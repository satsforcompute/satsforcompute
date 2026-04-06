use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;

use crate::btcpay::{self, BtcPayClient};
use crate::config::Config;
use crate::db::{self, Db};
use crate::models::{Order, OrderStatus};
use crate::templates;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Config,
    pub btcpay: std::sync::Arc<BtcPayClient>,
}

// GET /
pub async fn landing(State(state): State<AppState>) -> Html<String> {
    let plans = db::list_plans(&state.db);
    Html(templates::landing(&plans))
}

// POST /order
#[derive(Deserialize)]
pub struct OrderForm {
    github_handle: String,
    plan_id: String,
}

pub async fn create_order(State(state): State<AppState>, Form(form): Form<OrderForm>) -> Response {
    // Validate github handle
    let handle = form.github_handle.trim();
    if handle.is_empty() || !handle.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return (StatusCode::BAD_REQUEST, "invalid github handle").into_response();
    }

    let plan = match db::get_plan(&state.db, &form.plan_id) {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "invalid plan").into_response(),
    };

    let order_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let order = Order {
        id: order_id.clone(),
        github_handle: handle.to_string(),
        plan_id: plan.id.clone(),
        status: OrderStatus::PendingPayment,
        btcpay_invoice_id: None,
        price_sats: plan.price_sats,
        created_at: now,
        paid_at: None,
        provisioned_at: None,
        expires_at: None,
        error_message: None,
    };
    db::insert_order(&state.db, &order);

    // Create BTCPay invoice
    match state
        .btcpay
        .create_invoice(plan.price_sats, &order_id, handle)
        .await
    {
        Ok(invoice) => {
            db::update_order_invoice(&state.db, &order_id, &invoice.id);
            db::insert_event(
                &state.db,
                Some(&order_id),
                None,
                "invoice_created",
                Some(&invoice.id),
            );
            Redirect::to(&format!("/order/{order_id}")).into_response()
        }
        Err(e) => {
            tracing::error!(order_id = %order_id, error = %e, "failed to create invoice");
            db::update_order_failed(&state.db, &order_id, &e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("payment error: {e}"),
            )
                .into_response()
        }
    }
}

// GET /order/:id
pub async fn order_status(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let order = match db::get_order(&state.db, &id) {
        Some(o) => o,
        None => return (StatusCode::NOT_FOUND, "order not found").into_response(),
    };

    let node = db::get_node_for_order(&state.db, &id);
    let checkout_url = order
        .btcpay_invoice_id
        .as_deref()
        .map(|inv| state.btcpay.checkout_url(inv));

    Html(templates::order_status(
        &order,
        node.as_ref(),
        checkout_url.as_deref(),
    ))
    .into_response()
}

// GET /order/:id/status
pub async fn order_status_json(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match db::get_order(&state.db, &id) {
        Some(order) => {
            let node = db::get_node_for_order(&state.db, &id);
            axum::Json(serde_json::json!({
                "order": order,
                "node": node,
            }))
            .into_response()
        }
        None => (StatusCode::NOT_FOUND, "order not found").into_response(),
    }
}

// POST /webhooks/btcpay
pub async fn btcpay_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let signature = headers
        .get("btcpay-sig")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !state.btcpay.verify_webhook(&body, signature) {
        tracing::warn!("invalid webhook signature");
        return StatusCode::UNAUTHORIZED;
    }

    let payload = match btcpay::parse_webhook(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "failed to parse webhook");
            return StatusCode::BAD_REQUEST;
        }
    };

    tracing::info!(
        invoice_id = %payload.invoice_id,
        event = %payload.event_type,
        "webhook received"
    );

    // Only act on settled invoices
    if payload.event_type == "InvoiceSettled" || payload.event_type == "InvoicePaymentSettled" {
        if let Some(order) = db::find_order_by_invoice(&state.db, &payload.invoice_id) {
            if order.status == OrderStatus::PendingPayment {
                db::update_order_paid(&state.db, &order.id);
                db::insert_event(
                    &state.db,
                    Some(&order.id),
                    None,
                    "payment_settled",
                    Some(&payload.invoice_id),
                );
                tracing::info!(order_id = %order.id, "payment settled, queued for provisioning");
            }
        }
    }

    StatusCode::OK
}

// GET /health
pub async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "service": "dd-market",
    }))
}

// Admin routes (password protected)

fn check_admin(state: &AppState, headers: &HeaderMap) -> bool {
    let password = match &state.config.admin_password {
        Some(p) => p,
        None => return true, // no password = open admin (dev mode)
    };

    // Check Authorization: Bearer <password>
    if let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            return constant_time_eq(token.as_bytes(), password.as_bytes());
        }
    }

    // Check cookie
    if let Some(cookie) = headers.get("cookie").and_then(|v| v.to_str().ok()) {
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("dd_market_admin=") {
                return constant_time_eq(val.as_bytes(), password.as_bytes());
            }
        }
    }

    false
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// GET /admin/orders
pub async fn admin_orders(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !check_admin(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let orders = db::list_orders(&state.db);
    Html(templates::admin_orders(&orders)).into_response()
}

// GET /admin/nodes
pub async fn admin_nodes(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !check_admin(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let nodes = db::list_nodes(&state.db);
    Html(templates::admin_nodes(&nodes)).into_response()
}

// GET /admin/pool
pub async fn admin_pool(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !check_admin(&state, &headers) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let warm_local = db::count_warm_nodes(&state.db, "local");
    let warm_gcp = db::count_warm_nodes(&state.db, "gcp");
    let warm_nodes_local = db::list_warm_nodes(&state.db, "local");
    let warm_nodes_gcp = db::list_warm_nodes(&state.db, "gcp");
    let recent = db::recent_orders_summary(&state.db, 20);

    Html(templates::admin_pool(
        warm_local,
        warm_gcp,
        &warm_nodes_local,
        &warm_nodes_gcp,
        &recent,
        &state.config,
    ))
    .into_response()
}
