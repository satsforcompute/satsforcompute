use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};

use crate::models::{Node, NodeStatus, Order, OrderStatus, Plan, Provider};

pub type Db = Arc<Mutex<Connection>>;

pub fn open(path: &str) -> Db {
    let conn = Connection::open(path).expect("failed to open database");
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .expect("failed to set pragmas");
    Arc::new(Mutex::new(conn))
}

pub fn migrate(db: &Db) {
    let conn = db.lock().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS plans (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            machine_type TEXT NOT NULL,
            vcpus       INTEGER NOT NULL,
            ram_gb      INTEGER NOT NULL,
            disk_gb     INTEGER NOT NULL,
            duration_hours INTEGER NOT NULL,
            price_sats  INTEGER NOT NULL,
            provider    TEXT NOT NULL DEFAULT 'local',
            active      INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS orders (
            id              TEXT PRIMARY KEY,
            github_handle   TEXT NOT NULL,
            plan_id         TEXT NOT NULL REFERENCES plans(id),
            status          TEXT NOT NULL DEFAULT 'pending_payment',
            btcpay_invoice_id TEXT,
            price_sats      INTEGER NOT NULL,
            created_at      TEXT NOT NULL,
            paid_at         TEXT,
            provisioned_at  TEXT,
            expires_at      TEXT,
            error_message   TEXT
        );

        CREATE TABLE IF NOT EXISTS nodes (
            id              TEXT PRIMARY KEY,
            order_id        TEXT NOT NULL REFERENCES orders(id),
            github_handle   TEXT NOT NULL,
            provider        TEXT NOT NULL,
            vm_name         TEXT NOT NULL,
            hostname        TEXT,
            status          TEXT NOT NULL DEFAULT 'provisioning',
            created_at      TEXT NOT NULL,
            expires_at      TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            order_id    TEXT REFERENCES orders(id),
            node_id     TEXT REFERENCES nodes(id),
            event_type  TEXT NOT NULL,
            details     TEXT,
            created_at  TEXT NOT NULL
        );
        ",
    )
    .expect("failed to run migrations");
}

pub fn seed_plans(db: &Db) {
    let conn = db.lock().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM plans", [], |row| row.get(0))
        .unwrap_or(0);
    if count > 0 {
        return;
    }

    let plans = vec![
        // Local baremetal plans
        (
            "local-tiny-24h",
            "Tiny (24h)",
            "kvm-tiny",
            1,
            2,
            20,
            24,
            7_200,
            "local",
        ),
        (
            "local-small-24h",
            "Small (24h)",
            "kvm-small",
            2,
            4,
            40,
            24,
            12_000,
            "local",
        ),
        (
            "local-medium-24h",
            "Medium (24h)",
            "kvm-medium",
            4,
            8,
            80,
            24,
            24_000,
            "local",
        ),
        (
            "local-large-24h",
            "Large (24h)",
            "kvm-large",
            8,
            16,
            160,
            24,
            48_000,
            "local",
        ),
        (
            "local-small-720h",
            "Small (30d)",
            "kvm-small",
            2,
            4,
            40,
            720,
            360_000,
            "local",
        ),
        (
            "local-medium-720h",
            "Medium (30d)",
            "kvm-medium",
            4,
            8,
            80,
            720,
            720_000,
            "local",
        ),
        // GCP overflow plans
        (
            "gcp-tiny-24h",
            "GCP Tiny (24h)",
            "c3-standard-4",
            4,
            16,
            256,
            24,
            48_000,
            "gcp",
        ),
        (
            "gcp-standard-24h",
            "GCP Standard (24h)",
            "c3-standard-8",
            8,
            32,
            256,
            24,
            72_000,
            "gcp",
        ),
        (
            "gcp-tiny-720h",
            "GCP Tiny (30d)",
            "c3-standard-4",
            4,
            16,
            256,
            720,
            1_440_000,
            "gcp",
        ),
    ];

    for (id, name, machine_type, vcpus, ram_gb, disk_gb, duration_hours, price_sats, provider) in
        plans
    {
        conn.execute(
            "INSERT INTO plans (id, name, machine_type, vcpus, ram_gb, disk_gb, duration_hours, price_sats, provider) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, name, machine_type, vcpus, ram_gb, disk_gb, duration_hours, price_sats, provider],
        )
        .ok();
    }
}

pub fn list_plans(db: &Db) -> Vec<Plan> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, name, machine_type, vcpus, ram_gb, disk_gb, duration_hours, price_sats, provider, active FROM plans WHERE active = 1 ORDER BY provider, price_sats")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Plan {
            id: row.get(0)?,
            name: row.get(1)?,
            machine_type: row.get(2)?,
            vcpus: row.get(3)?,
            ram_gb: row.get(4)?,
            disk_gb: row.get(5)?,
            duration_hours: row.get(6)?,
            price_sats: row.get(7)?,
            provider: Provider::from_str(&row.get::<_, String>(8)?),
            active: row.get(9)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn get_plan(db: &Db, id: &str) -> Option<Plan> {
    let conn = db.lock().unwrap();
    conn.query_row(
        "SELECT id, name, machine_type, vcpus, ram_gb, disk_gb, duration_hours, price_sats, provider, active FROM plans WHERE id = ?1",
        params![id],
        |row| {
            Ok(Plan {
                id: row.get(0)?,
                name: row.get(1)?,
                machine_type: row.get(2)?,
                vcpus: row.get(3)?,
                ram_gb: row.get(4)?,
                disk_gb: row.get(5)?,
                duration_hours: row.get(6)?,
                price_sats: row.get(7)?,
                provider: Provider::from_str(&row.get::<_, String>(8)?),
                active: row.get(9)?,
            })
        },
    )
    .ok()
}

pub fn insert_order(db: &Db, order: &Order) {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO orders (id, github_handle, plan_id, status, btcpay_invoice_id, price_sats, created_at, paid_at, provisioned_at, expires_at, error_message) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            order.id,
            order.github_handle,
            order.plan_id,
            order.status.as_str(),
            order.btcpay_invoice_id,
            order.price_sats,
            order.created_at,
            order.paid_at,
            order.provisioned_at,
            order.expires_at,
            order.error_message,
        ],
    )
    .expect("failed to insert order");
}

pub fn get_order(db: &Db, id: &str) -> Option<Order> {
    let conn = db.lock().unwrap();
    conn.query_row(
        "SELECT id, github_handle, plan_id, status, btcpay_invoice_id, price_sats, created_at, paid_at, provisioned_at, expires_at, error_message FROM orders WHERE id = ?1",
        params![id],
        |row| {
            Ok(Order {
                id: row.get(0)?,
                github_handle: row.get(1)?,
                plan_id: row.get(2)?,
                status: OrderStatus::from_str(&row.get::<_, String>(3)?),
                btcpay_invoice_id: row.get(4)?,
                price_sats: row.get(5)?,
                created_at: row.get(6)?,
                paid_at: row.get(7)?,
                provisioned_at: row.get(8)?,
                expires_at: row.get(9)?,
                error_message: row.get(10)?,
            })
        },
    )
    .ok()
}

pub fn update_order_status(db: &Db, id: &str, status: OrderStatus) {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE orders SET status = ?1 WHERE id = ?2",
        params![status.as_str(), id],
    )
    .ok();
}

pub fn update_order_invoice(db: &Db, id: &str, invoice_id: &str) {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE orders SET btcpay_invoice_id = ?1 WHERE id = ?2",
        params![invoice_id, id],
    )
    .ok();
}

pub fn update_order_paid(db: &Db, id: &str) {
    let conn = db.lock().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE orders SET status = 'paid', paid_at = ?1 WHERE id = ?2",
        params![now, id],
    )
    .ok();
}

pub fn update_order_provisioned(db: &Db, order_id: &str, expires_at: &str) {
    let conn = db.lock().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE orders SET status = 'active', provisioned_at = ?1, expires_at = ?2 WHERE id = ?3",
        params![now, expires_at, order_id],
    )
    .ok();
}

pub fn update_order_failed(db: &Db, id: &str, error: &str) {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE orders SET status = 'failed', error_message = ?1 WHERE id = ?2",
        params![error, id],
    )
    .ok();
}

pub fn list_orders(db: &Db) -> Vec<Order> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, github_handle, plan_id, status, btcpay_invoice_id, price_sats, created_at, paid_at, provisioned_at, expires_at, error_message FROM orders ORDER BY created_at DESC")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Order {
            id: row.get(0)?,
            github_handle: row.get(1)?,
            plan_id: row.get(2)?,
            status: OrderStatus::from_str(&row.get::<_, String>(3)?),
            btcpay_invoice_id: row.get(4)?,
            price_sats: row.get(5)?,
            created_at: row.get(6)?,
            paid_at: row.get(7)?,
            provisioned_at: row.get(8)?,
            expires_at: row.get(9)?,
            error_message: row.get(10)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn list_paid_orders(db: &Db) -> Vec<Order> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, github_handle, plan_id, status, btcpay_invoice_id, price_sats, created_at, paid_at, provisioned_at, expires_at, error_message FROM orders WHERE status = 'paid'")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Order {
            id: row.get(0)?,
            github_handle: row.get(1)?,
            plan_id: row.get(2)?,
            status: OrderStatus::from_str(&row.get::<_, String>(3)?),
            btcpay_invoice_id: row.get(4)?,
            price_sats: row.get(5)?,
            created_at: row.get(6)?,
            paid_at: row.get(7)?,
            provisioned_at: row.get(8)?,
            expires_at: row.get(9)?,
            error_message: row.get(10)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn find_order_by_invoice(db: &Db, invoice_id: &str) -> Option<Order> {
    let conn = db.lock().unwrap();
    conn.query_row(
        "SELECT id, github_handle, plan_id, status, btcpay_invoice_id, price_sats, created_at, paid_at, provisioned_at, expires_at, error_message FROM orders WHERE btcpay_invoice_id = ?1",
        params![invoice_id],
        |row| {
            Ok(Order {
                id: row.get(0)?,
                github_handle: row.get(1)?,
                plan_id: row.get(2)?,
                status: OrderStatus::from_str(&row.get::<_, String>(3)?),
                btcpay_invoice_id: row.get(4)?,
                price_sats: row.get(5)?,
                created_at: row.get(6)?,
                paid_at: row.get(7)?,
                provisioned_at: row.get(8)?,
                expires_at: row.get(9)?,
                error_message: row.get(10)?,
            })
        },
    )
    .ok()
}

pub fn insert_node(db: &Db, node: &Node) {
    let conn = db.lock().unwrap();
    conn.execute(
        "INSERT INTO nodes (id, order_id, github_handle, provider, vm_name, hostname, status, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            node.id,
            node.order_id,
            node.github_handle,
            node.provider.as_str(),
            node.vm_name,
            node.hostname,
            node.status.as_str(),
            node.created_at,
            node.expires_at,
        ],
    )
    .expect("failed to insert node");
}

pub fn update_node_status(db: &Db, id: &str, status: NodeStatus) {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE nodes SET status = ?1 WHERE id = ?2",
        params![status.as_str(), id],
    )
    .ok();
}

#[allow(dead_code)]
pub fn update_node_hostname(db: &Db, id: &str, hostname: &str) {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE nodes SET hostname = ?1 WHERE id = ?2",
        params![hostname, id],
    )
    .ok();
}

pub fn list_nodes(db: &Db) -> Vec<Node> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, order_id, github_handle, provider, vm_name, hostname, status, created_at, expires_at FROM nodes ORDER BY created_at DESC")
        .unwrap();
    stmt.query_map([], |row| {
        Ok(Node {
            id: row.get(0)?,
            order_id: row.get(1)?,
            github_handle: row.get(2)?,
            provider: Provider::from_str(&row.get::<_, String>(3)?),
            vm_name: row.get(4)?,
            hostname: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: row.get(7)?,
            expires_at: row.get(8)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn get_node_for_order(db: &Db, order_id: &str) -> Option<Node> {
    let conn = db.lock().unwrap();
    conn.query_row(
        "SELECT id, order_id, github_handle, provider, vm_name, hostname, status, created_at, expires_at FROM nodes WHERE order_id = ?1",
        params![order_id],
        |row| {
            Ok(Node {
                id: row.get(0)?,
                order_id: row.get(1)?,
                github_handle: row.get(2)?,
                provider: Provider::from_str(&row.get::<_, String>(3)?),
                vm_name: row.get(4)?,
                hostname: row.get(5)?,
                status: NodeStatus::from_str(&row.get::<_, String>(6)?),
                created_at: row.get(7)?,
                expires_at: row.get(8)?,
            })
        },
    )
    .ok()
}

pub fn list_expired_nodes(db: &Db) -> Vec<Node> {
    let conn = db.lock().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn
        .prepare("SELECT id, order_id, github_handle, provider, vm_name, hostname, status, created_at, expires_at FROM nodes WHERE status = 'running' AND expires_at < ?1")
        .unwrap();
    stmt.query_map(params![now], |row| {
        Ok(Node {
            id: row.get(0)?,
            order_id: row.get(1)?,
            github_handle: row.get(2)?,
            provider: Provider::from_str(&row.get::<_, String>(3)?),
            vm_name: row.get(4)?,
            hostname: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: row.get(7)?,
            expires_at: row.get(8)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

// --- Warm pool queries ---

pub fn count_warm_nodes(db: &Db, provider: &str) -> i64 {
    let conn = db.lock().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE status = 'warm' AND provider = ?1",
        params![provider],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

pub fn list_warm_nodes(db: &Db, provider: &str) -> Vec<Node> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id, order_id, github_handle, provider, vm_name, hostname, status, created_at, expires_at FROM nodes WHERE status = 'warm' AND provider = ?1 ORDER BY created_at ASC")
        .unwrap();
    stmt.query_map(params![provider], |row| {
        Ok(Node {
            id: row.get(0)?,
            order_id: row.get(1)?,
            github_handle: row.get(2)?,
            provider: Provider::from_str(&row.get::<_, String>(3)?),
            vm_name: row.get(4)?,
            hostname: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: row.get(7)?,
            expires_at: row.get(8)?,
        })
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn claim_warm_node(
    db: &Db,
    node_id: &str,
    order_id: &str,
    github_handle: &str,
    expires_at: &str,
) {
    let conn = db.lock().unwrap();
    conn.execute(
        "UPDATE nodes SET status = 'running', order_id = ?1, github_handle = ?2, expires_at = ?3 WHERE id = ?4",
        params![order_id, github_handle, expires_at, node_id],
    )
    .ok();
}

/// Get a snapshot of recent order history for LLM context.
/// Returns JSON-serializable summary of the last N orders.
pub fn recent_orders_summary(db: &Db, limit: usize) -> Vec<serde_json::Value> {
    let conn = db.lock().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT o.status, o.plan_id, o.paid_at, o.created_at, p.provider, p.name
             FROM orders o JOIN plans p ON o.plan_id = p.id
             ORDER BY o.created_at DESC LIMIT ?1",
        )
        .unwrap();
    stmt.query_map(params![limit as i64], |row| {
        Ok(serde_json::json!({
            "status": row.get::<_, String>(0)?,
            "plan_id": row.get::<_, String>(1)?,
            "paid_at": row.get::<_, Option<String>>(2)?,
            "created_at": row.get::<_, String>(3)?,
            "provider": row.get::<_, String>(4)?,
            "plan_name": row.get::<_, String>(5)?,
        }))
    })
    .unwrap()
    .filter_map(|r| r.ok())
    .collect()
}

pub fn insert_event(
    db: &Db,
    order_id: Option<&str>,
    node_id: Option<&str>,
    event_type: &str,
    details: Option<&str>,
) {
    let conn = db.lock().unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO events (order_id, node_id, event_type, details, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![order_id, node_id, event_type, details, now],
    )
    .ok();
}
