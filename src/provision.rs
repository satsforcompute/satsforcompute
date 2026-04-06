use chrono::Utc;

use crate::config::Config;
use crate::db::{self, Db};
use crate::models::{Node, NodeStatus, Order, OrderStatus, Provider};
use crate::{gcp, local};

pub async fn provision_order(db: &Db, config: &Config, order: &Order) {
    tracing::info!(order_id = %order.id, github = %order.github_handle, "provisioning order");

    db::update_order_status(db, &order.id, OrderStatus::Provisioning);

    let plan = match db::get_plan(db, &order.plan_id) {
        Some(p) => p,
        None => {
            db::update_order_failed(db, &order.id, "plan not found");
            return;
        }
    };

    let now = Utc::now();
    let expires_at = now + chrono::Duration::hours(plan.duration_hours);

    // Try to claim a warm node first
    let provider_str = plan.provider.as_str();
    let warm_nodes = db::list_warm_nodes(db, provider_str);

    if let Some(warm) = warm_nodes.first() {
        tracing::info!(
            order_id = %order.id,
            node_id = %warm.id,
            vm = %warm.vm_name,
            "claiming warm node"
        );

        // Reassign the VM to the customer
        let reassign_result = match warm.provider {
            Provider::Local => {
                local::reassign_vm(config, &warm.vm_name, &order.github_handle).await
            }
            Provider::Gcp => {
                // GCP warm nodes: can't easily reassign in-place, so delete and recreate
                // (or we could SSH in like local -- for now, treat as cold provision)
                Err("gcp warm reassign not implemented, falling through".into())
            }
        };

        if reassign_result.is_ok() {
            db::claim_warm_node(
                db,
                &warm.id,
                &order.id,
                &order.github_handle,
                &expires_at.to_rfc3339(),
            );
            db::update_order_provisioned(db, &order.id, &expires_at.to_rfc3339());
            db::insert_event(
                db,
                Some(&order.id),
                Some(&warm.id),
                "warm_claimed",
                Some(&format!("vm={}", warm.vm_name)),
            );
            tracing::info!(order_id = %order.id, vm = %warm.vm_name, "warm node claimed, instant delivery");
            return;
        }
        // If reassign failed, fall through to cold provision
        tracing::warn!(order_id = %order.id, "warm claim failed, provisioning fresh");
    }

    // Cold provision: create a new VM from scratch
    let vm_name = format!("dd-mkt-{}", &order.id[..8]);

    let (provider, result) = match plan.provider {
        Provider::Local => {
            let has_capacity = local::check_capacity(config).await.unwrap_or(false);
            if has_capacity {
                let size = local::plan_to_vm_size(&plan.machine_type);
                let r = local::create_vm(config, &vm_name, size, &order.github_handle).await;
                (Provider::Local, r)
            } else {
                tracing::warn!("local at capacity, falling back to GCP");
                let script = gcp::generate_startup_script(config, &order.github_handle, &vm_name);
                let r = gcp::create_instance(
                    &reqwest::Client::new(),
                    config,
                    &vm_name,
                    "c3-standard-4",
                    256,
                    &script,
                    &order.github_handle,
                )
                .await;
                (Provider::Gcp, r)
            }
        }
        Provider::Gcp => {
            let script = gcp::generate_startup_script(config, &order.github_handle, &vm_name);
            let r = gcp::create_instance(
                &reqwest::Client::new(),
                config,
                &vm_name,
                &plan.machine_type,
                plan.disk_gb,
                &script,
                &order.github_handle,
            )
            .await;
            (Provider::Gcp, r)
        }
    };

    match result {
        Ok(()) => {
            let node = Node {
                id: uuid::Uuid::new_v4().to_string(),
                order_id: order.id.clone(),
                github_handle: order.github_handle.clone(),
                provider,
                vm_name: vm_name.clone(),
                hostname: None,
                status: NodeStatus::Running,
                created_at: now.to_rfc3339(),
                expires_at: expires_at.to_rfc3339(),
            };
            db::insert_node(db, &node);
            db::update_order_provisioned(db, &order.id, &expires_at.to_rfc3339());
            db::insert_event(
                db,
                Some(&order.id),
                Some(&node.id),
                "provisioned",
                Some(&format!("provider={}, vm={vm_name}", provider.as_str())),
            );
            tracing::info!(order_id = %order.id, vm = %vm_name, "node provisioned (cold)");
        }
        Err(e) => {
            db::update_order_failed(db, &order.id, &e);
            db::insert_event(db, Some(&order.id), None, "provision_failed", Some(&e));
            tracing::error!(order_id = %order.id, error = %e, "provisioning failed");
        }
    }
}

/// Background loop: pick up paid orders and provision them.
pub async fn provisioner_loop(db: Db, config: Config) {
    loop {
        let paid = db::list_paid_orders(&db);
        for order in paid {
            provision_order(&db, &config, &order).await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}
