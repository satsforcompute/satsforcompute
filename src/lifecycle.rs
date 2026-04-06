use crate::config::Config;
use crate::db::{self, Db};
use crate::models::{NodeStatus, OrderStatus, Provider};
use crate::{gcp, local};

pub async fn lifecycle_loop(db: Db, config: Config) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        interval.tick().await;
        if let Err(e) = check_expirations(&db, &config).await {
            tracing::error!(error = %e, "lifecycle check failed");
        }
    }
}

async fn check_expirations(db: &Db, config: &Config) -> Result<(), String> {
    let expired = db::list_expired_nodes(db);
    if expired.is_empty() {
        return Ok(());
    }

    tracing::info!(count = expired.len(), "processing expired nodes");

    for node in expired {
        tracing::info!(node_id = %node.id, vm = %node.vm_name, "tearing down expired node");

        let result = match node.provider {
            Provider::Local => local::destroy_vm(config, &node.vm_name).await,
            Provider::Gcp => gcp::delete_instance(config, &node.vm_name).await,
        };

        match result {
            Ok(()) => {
                db::update_node_status(db, &node.id, NodeStatus::Deleted);
                db::update_order_status(db, &node.order_id, OrderStatus::Expired);
                db::insert_event(
                    db,
                    Some(&node.order_id),
                    Some(&node.id),
                    "expired_teardown",
                    None,
                );
                tracing::info!(node_id = %node.id, "node deleted");
            }
            Err(e) => {
                tracing::error!(node_id = %node.id, error = %e, "teardown failed");
                db::insert_event(
                    db,
                    Some(&node.order_id),
                    Some(&node.id),
                    "teardown_failed",
                    Some(&e),
                );
            }
        }
    }

    Ok(())
}
