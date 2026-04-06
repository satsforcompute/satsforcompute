use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::db::{self, Db};
use crate::models::{Node, NodeStatus, Provider};
use crate::{gcp, local};

#[derive(Debug, Serialize)]
struct PoolState {
    timestamp: String,
    warm_nodes_local: i64,
    warm_nodes_gcp: i64,
    active_nodes: usize,
    recent_orders: Vec<serde_json::Value>,
    local_capacity_available: bool,
    pool_max: usize,
}

#[derive(Debug, Deserialize)]
struct PoolPlan {
    actions: Vec<PoolAction>,
    reasoning: String,
}

#[derive(Debug, Deserialize)]
struct PoolAction {
    action: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    node_id: Option<String>,
}

const SYSTEM_PROMPT: &str = r#"You are a capacity planner for a TDX node marketplace. Users pay Bitcoin to get confidential computing VMs.

Your job: decide how many warm (pre-provisioned, unassigned) VMs to keep ready so customers get instant allocation when they pay.

You will receive the current pool state as JSON. Respond with a JSON object containing:
- "actions": array of actions to take
- "reasoning": one sentence explaining your decision

Each action is one of:
- {"action": "create_warm", "provider": "local", "size": "small"}
- {"action": "create_warm", "provider": "gcp", "size": "tiny"}
- {"action": "destroy_warm", "provider": "local", "node_id": "..."}
- {"action": "none"}

Local sizes: tiny, small, medium, large
GCP sizes: tiny (c3-standard-4), standard (c3-standard-8)

Guidelines:
- Prefer local VMs (cheaper). Only pre-provision GCP if local is at capacity.
- Keep at least 1 warm local node if there's been any recent demand.
- Scale warm pool with demand: more orders recently = more warm nodes.
- Don't exceed pool_max total warm nodes.
- If no orders in weeks, scale down to 0 to save resources.
- Small is the most popular size. Default to small unless demand data shows otherwise.
- Consider time patterns: if most orders come during business hours and it's late night, fewer warm nodes needed.
- Warm GCP nodes cost real money. Only keep them warm if demand justifies it.

Respond with ONLY the JSON object, no markdown fences."#;

pub async fn pool_loop(db: Db, config: Config) {
    let client = reqwest::Client::new();
    let interval = std::time::Duration::from_secs(config.pool_interval_secs);

    tracing::info!(
        interval_secs = config.pool_interval_secs,
        model = %config.openrouter_model,
        "LLM pool planner started"
    );

    loop {
        if let Err(e) = run_plan_cycle(&db, &config, &client).await {
            tracing::error!(error = %e, "pool plan cycle failed");
        }
        tokio::time::sleep(interval).await;
    }
}

async fn run_plan_cycle(db: &Db, config: &Config, client: &reqwest::Client) -> Result<(), String> {
    let state = gather_state(db, config).await;
    let state_json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;

    tracing::info!(
        warm_local = state.warm_nodes_local,
        warm_gcp = state.warm_nodes_gcp,
        active = state.active_nodes,
        recent_orders = state.recent_orders.len(),
        "pool state"
    );

    let plan = ask_llm(client, config, &state_json).await?;

    tracing::info!(
        reasoning = %plan.reasoning,
        actions = plan.actions.len(),
        "LLM pool plan"
    );

    for action in &plan.actions {
        if let Err(e) = execute_action(db, config, action).await {
            tracing::error!(error = %e, action = ?action.action, "pool action failed");
            db::insert_event(db, None, None, "pool_action_failed", Some(&e));
        }
    }

    Ok(())
}

async fn gather_state(db: &Db, config: &Config) -> PoolState {
    let warm_local = db::count_warm_nodes(db, "local");
    let warm_gcp = db::count_warm_nodes(db, "gcp");
    let all_nodes = db::list_nodes(db);
    let active_nodes = all_nodes
        .iter()
        .filter(|n| n.status == NodeStatus::Running)
        .count();
    let recent_orders = db::recent_orders_summary(db, 50);
    let local_cap = local::check_capacity(config).await.unwrap_or(false);

    PoolState {
        timestamp: Utc::now().to_rfc3339(),
        warm_nodes_local: warm_local,
        warm_nodes_gcp: warm_gcp,
        active_nodes,
        recent_orders,
        local_capacity_available: local_cap,
        pool_max: config.pool_max,
    }
}

async fn ask_llm(
    client: &reqwest::Client,
    config: &Config,
    state_json: &str,
) -> Result<PoolPlan, String> {
    let body = serde_json::json!({
        "model": config.openrouter_model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": format!("Current pool state:\n{state_json}")},
        ],
        "temperature": 0.2,
        "max_tokens": 512,
    });

    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header(
            "Authorization",
            format!("Bearer {}", config.openrouter_api_key),
        )
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("openrouter request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("openrouter returned {status}: {text}"));
    }

    let response: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("openrouter parse failed: {e}"))?;

    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("no content in LLM response")?;

    let clean = content
        .trim()
        .strip_prefix("```json")
        .unwrap_or(content.trim())
        .strip_prefix("```")
        .unwrap_or(content.trim())
        .strip_suffix("```")
        .unwrap_or(content.trim())
        .trim();

    serde_json::from_str::<PoolPlan>(clean)
        .map_err(|e| format!("failed to parse LLM plan: {e}\nraw: {content}"))
}

async fn execute_action(db: &Db, config: &Config, action: &PoolAction) -> Result<(), String> {
    match action.action.as_str() {
        "create_warm" => {
            let vm_name = format!("warm-{}", &uuid::Uuid::new_v4().to_string()[..8]);
            let provider = Provider::from_str(&action.provider);

            tracing::info!(vm = %vm_name, provider = %action.provider, size = %action.size, "creating warm node");

            match provider {
                Provider::Local => {
                    local::create_vm(config, &vm_name, &action.size, "unclaimed").await?;
                }
                Provider::Gcp => {
                    let machine_type = match action.size.as_str() {
                        "standard" => "c3-standard-8",
                        _ => "c3-standard-4",
                    };
                    let script = gcp::generate_startup_script(config, "unclaimed", &vm_name);
                    gcp::create_instance(config, &vm_name, machine_type, 256, &script, "unclaimed")
                        .await?;
                }
            }

            let node = Node {
                id: uuid::Uuid::new_v4().to_string(),
                order_id: String::new(),
                github_handle: "unclaimed".into(),
                provider,
                vm_name,
                hostname: None,
                status: NodeStatus::Warm,
                created_at: Utc::now().to_rfc3339(),
                expires_at: "2099-01-01T00:00:00Z".into(),
            };
            db::insert_node(db, &node);
            db::insert_event(
                db,
                None,
                Some(&node.id),
                "warm_created",
                Some(provider.as_str()),
            );
            Ok(())
        }
        "destroy_warm" => {
            let node_id = action
                .node_id
                .as_deref()
                .ok_or("destroy_warm requires node_id")?;

            let warm = db::list_warm_nodes(db, &action.provider);
            let target = warm.iter().find(|n| n.id == node_id).or(warm.first());

            if let Some(node) = target {
                tracing::info!(vm = %node.vm_name, "destroying warm node");
                match node.provider {
                    Provider::Local => {
                        local::destroy_vm(config, &node.vm_name).await?;
                    }
                    Provider::Gcp => {
                        gcp::delete_instance(config, &node.vm_name).await?;
                    }
                }
                db::update_node_status(db, &node.id, NodeStatus::Deleted);
                db::insert_event(db, None, Some(&node.id), "warm_destroyed", None);
            }
            Ok(())
        }
        "none" => Ok(()),
        other => {
            tracing::warn!(action = %other, "unknown pool action, ignoring");
            Ok(())
        }
    }
}
