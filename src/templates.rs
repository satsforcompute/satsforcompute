use crate::config::Config;
use crate::models::{Node, Order, Plan};

const STYLE: &str = r#"
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: -apple-system, system-ui, sans-serif; background: #0a0a0a; color: #e0e0e0; }
  .container { max-width: 800px; margin: 0 auto; padding: 2rem; }
  h1 { font-size: 2rem; margin-bottom: 0.5rem; }
  h2 { font-size: 1.4rem; margin: 1.5rem 0 0.5rem; }
  p { margin: 0.5rem 0; color: #999; }
  a { color: #60a5fa; text-decoration: none; }
  a:hover { text-decoration: underline; }
  .card { background: #1a1a1a; border: 1px solid #333; border-radius: 8px; padding: 1.5rem; margin: 1rem 0; }
  .plans { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 1rem; }
  .plan { cursor: pointer; transition: border-color 0.2s; }
  .plan:hover, .plan.selected { border-color: #60a5fa; }
  .plan h3 { font-size: 1.1rem; margin-bottom: 0.5rem; }
  .plan .specs { font-size: 0.85rem; color: #888; }
  .plan .price { font-size: 1.2rem; font-weight: bold; color: #f59e0b; margin-top: 0.5rem; }
  .plan .provider { font-size: 0.75rem; color: #666; text-transform: uppercase; }
  input, select, button { font-size: 1rem; padding: 0.6rem 1rem; border-radius: 6px; border: 1px solid #333; background: #111; color: #e0e0e0; }
  input:focus, select:focus { outline: none; border-color: #60a5fa; }
  button { background: #2563eb; border: none; cursor: pointer; font-weight: 600; }
  button:hover { background: #1d4ed8; }
  .form-group { margin: 1rem 0; }
  label { display: block; margin-bottom: 0.3rem; font-weight: 500; }
  table { width: 100%; border-collapse: collapse; margin: 1rem 0; }
  th, td { padding: 0.6rem; text-align: left; border-bottom: 1px solid #222; }
  th { color: #888; font-weight: 500; }
  .badge { display: inline-block; padding: 0.2rem 0.6rem; border-radius: 4px; font-size: 0.8rem; font-weight: 600; }
  .badge-green { background: #064e3b; color: #34d399; }
  .badge-yellow { background: #78350f; color: #fbbf24; }
  .badge-red { background: #7f1d1d; color: #f87171; }
  .badge-blue { background: #1e3a5f; color: #60a5fa; }
  .badge-gray { background: #333; color: #999; }
  .header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 2rem; border-bottom: 1px solid #222; padding-bottom: 1rem; }
  .header nav a { margin-left: 1.5rem; }
</style>
"#;

fn status_badge(status: &str) -> String {
    let class = match status {
        "active" | "running" => "badge-green",
        "paid" | "provisioning" => "badge-blue",
        "pending_payment" => "badge-yellow",
        "expired" | "stopped" => "badge-gray",
        "failed" | "deleted" => "badge-red",
        _ => "badge-gray",
    };
    format!(r#"<span class="badge {class}">{status}</span>"#)
}

fn format_sats(sats: i64) -> String {
    if sats >= 1_000_000 {
        format!("{:.2}M sats", sats as f64 / 1_000_000.0)
    } else if sats >= 1_000 {
        format!("{}k sats", sats / 1_000)
    } else {
        format!("{sats} sats")
    }
}

pub fn landing(plans: &[Plan]) -> String {
    let mut local_plans = String::new();
    let mut gcp_plans = String::new();

    for plan in plans {
        let card = format!(
            r#"<div class="card plan" onclick="selectPlan('{id}')">
                <div class="provider">{provider}</div>
                <h3>{name}</h3>
                <div class="specs">{vcpus} vCPU &middot; {ram}GB RAM &middot; {disk}GB disk &middot; {hours}h</div>
                <div class="price">{price}</div>
            </div>"#,
            id = plan.id,
            provider = plan.provider.as_str(),
            name = plan.name,
            vcpus = plan.vcpus,
            ram = plan.ram_gb,
            disk = plan.disk_gb,
            hours = plan.duration_hours,
            price = format_sats(plan.price_sats),
        );
        match plan.provider {
            crate::models::Provider::Local => local_plans.push_str(&card),
            crate::models::Provider::Gcp => gcp_plans.push_str(&card),
        }
    }

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>DD Market — TDX Node Marketplace</title>{STYLE}</head>
<body>
<div class="container">
  <div class="header">
    <h1>DD Market</h1>
    <nav><a href="/admin/orders">Admin</a></nav>
  </div>
  <p>Get a confidential TDX node with dd-agent, assigned to your GitHub identity. Pay with Bitcoin.</p>

  <form method="POST" action="/order" id="orderForm">
    <div class="form-group">
      <label for="github_handle">GitHub username or org</label>
      <input type="text" id="github_handle" name="github_handle" required pattern="[a-zA-Z0-9][a-zA-Z0-9-]*" placeholder="your-github-handle" style="width:100%">
    </div>

    <h2>Baremetal Nodes</h2>
    <div class="plans">{local_plans}</div>

    <h2>Cloud Nodes (GCP)</h2>
    <div class="plans">{gcp_plans}</div>

    <input type="hidden" id="plan_id" name="plan_id" required>
    <div style="margin-top:1.5rem">
      <button type="submit">Pay with Bitcoin</button>
    </div>
  </form>
</div>
<script>
function selectPlan(id) {{
  document.getElementById('plan_id').value = id;
  document.querySelectorAll('.plan').forEach(p => p.classList.remove('selected'));
  event.currentTarget.classList.add('selected');
}}
</script>
</body></html>"#
    )
}

pub fn order_status(order: &Order, node: Option<&Node>, checkout_url: Option<&str>) -> String {
    let status_html = status_badge(order.status.as_str());

    let payment_section = if let Some(url) = checkout_url {
        format!(
            r#"<div class="card">
                <h2>Payment</h2>
                <p>Amount: <strong>{price}</strong></p>
                <p><a href="{url}" target="_blank">Open payment page &rarr;</a></p>
                <iframe src="{url}" style="width:100%;height:400px;border:1px solid #333;border-radius:8px;margin-top:1rem" allow="clipboard-write"></iframe>
            </div>"#,
            price = format_sats(order.price_sats),
        )
    } else {
        String::new()
    };

    let node_section = if let Some(n) = node {
        let hostname_link = n
            .hostname
            .as_deref()
            .map(|h| {
                format!(r#"<p>URL: <a href="https://{h}" target="_blank">https://{h}</a></p>"#)
            })
            .unwrap_or_default();
        format!(
            r#"<div class="card">
                <h2>Node</h2>
                <p>VM: <code>{vm}</code> &middot; Provider: {provider} &middot; Status: {node_status}</p>
                {hostname_link}
                <p>Expires: {expires}</p>
            </div>"#,
            vm = n.vm_name,
            provider = n.provider.as_str(),
            node_status = status_badge(n.status.as_str()),
            expires = n.expires_at,
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Order {id} — DD Market</title>{STYLE}
<meta http-equiv="refresh" content="15">
</head>
<body>
<div class="container">
  <div class="header">
    <h1>DD Market</h1>
    <nav><a href="/">Home</a></nav>
  </div>
  <div class="card">
    <h2>Order {short_id}</h2>
    <p>Status: {status_html}</p>
    <p>GitHub: <strong>{github}</strong></p>
    <p>Plan: {plan}</p>
    <p>Created: {created}</p>
  </div>
  {payment_section}
  {node_section}
</div>
</body></html>"#,
        id = order.id,
        short_id = &order.id[..8],
        github = order.github_handle,
        plan = order.plan_id,
        created = order.created_at,
    )
}

pub fn admin_orders(orders: &[Order]) -> String {
    let mut rows = String::new();
    for o in orders {
        rows.push_str(&format!(
            r#"<tr>
                <td><a href="/order/{id}">{short}</a></td>
                <td>{github}</td>
                <td>{plan}</td>
                <td>{status}</td>
                <td>{price}</td>
                <td>{created}</td>
            </tr>"#,
            id = o.id,
            short = &o.id[..8],
            github = o.github_handle,
            plan = o.plan_id,
            status = status_badge(o.status.as_str()),
            price = format_sats(o.price_sats),
            created = o.created_at,
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Admin — DD Market</title>{STYLE}</head>
<body>
<div class="container">
  <div class="header">
    <h1>Admin</h1>
    <nav><a href="/">Home</a> <a href="/admin/nodes">Nodes</a> <a href="/admin/pool">Pool</a></nav>
  </div>
  <h2>Orders</h2>
  <table>
    <tr><th>ID</th><th>GitHub</th><th>Plan</th><th>Status</th><th>Price</th><th>Created</th></tr>
    {rows}
  </table>
</div>
</body></html>"#
    )
}

pub fn admin_nodes(nodes: &[Node]) -> String {
    let mut rows = String::new();
    for n in nodes {
        let hostname = n.hostname.as_deref().unwrap_or("-");
        rows.push_str(&format!(
            r#"<tr>
                <td>{short}</td>
                <td>{github}</td>
                <td>{vm}</td>
                <td>{provider}</td>
                <td>{hostname}</td>
                <td>{status}</td>
                <td>{expires}</td>
            </tr>"#,
            short = &n.id[..8],
            github = n.github_handle,
            vm = n.vm_name,
            provider = n.provider.as_str(),
            status = status_badge(n.status.as_str()),
            expires = n.expires_at,
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Nodes — DD Market</title>{STYLE}</head>
<body>
<div class="container">
  <div class="header">
    <h1>Admin</h1>
    <nav><a href="/">Home</a> <a href="/admin/orders">Orders</a> <a href="/admin/pool">Pool</a></nav>
  </div>
  <h2>Nodes</h2>
  <table>
    <tr><th>ID</th><th>GitHub</th><th>VM</th><th>Provider</th><th>Hostname</th><th>Status</th><th>Expires</th></tr>
    {rows}
  </table>
</div>
</body></html>"#
    )
}

pub fn admin_pool(
    warm_local: i64,
    warm_gcp: i64,
    warm_nodes_local: &[Node],
    warm_nodes_gcp: &[Node],
    recent_orders: &[serde_json::Value],
    config: &Config,
) -> String {
    let mut warm_rows = String::new();
    for n in warm_nodes_local.iter().chain(warm_nodes_gcp.iter()) {
        warm_rows.push_str(&format!(
            r#"<tr>
                <td>{short}</td>
                <td>{vm}</td>
                <td>{provider}</td>
                <td>{status}</td>
                <td>{created}</td>
            </tr>"#,
            short = &n.id[..8],
            vm = n.vm_name,
            provider = n.provider.as_str(),
            status = status_badge(n.status.as_str()),
            created = n.created_at,
        ));
    }

    let mut order_rows = String::new();
    for o in recent_orders {
        order_rows.push_str(&format!(
            r#"<tr>
                <td>{plan}</td>
                <td>{provider}</td>
                <td>{status}</td>
                <td>{paid}</td>
            </tr>"#,
            plan = o["plan_name"].as_str().unwrap_or("-"),
            provider = o["provider"].as_str().unwrap_or("-"),
            status = status_badge(o["status"].as_str().unwrap_or("-")),
            paid = o["paid_at"].as_str().unwrap_or("-"),
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Pool — DD Market</title>{STYLE}
<meta http-equiv="refresh" content="30">
</head>
<body>
<div class="container">
  <div class="header">
    <h1>Admin</h1>
    <nav><a href="/">Home</a> <a href="/admin/orders">Orders</a> <a href="/admin/nodes">Nodes</a></nav>
  </div>

  <h2>Warm Pool</h2>
  <div class="card">
    <p>Model: <strong>{model}</strong> &middot; Max: <strong>{max}</strong> &middot; Interval: <strong>{interval}s</strong></p>
    <p>Warm local: <strong>{warm_local}</strong> &middot; Warm GCP: <strong>{warm_gcp}</strong></p>
  </div>

  <h2>Warm Nodes</h2>
  <table>
    <tr><th>ID</th><th>VM</th><th>Provider</th><th>Status</th><th>Created</th></tr>
    {warm_rows}
  </table>

  <h2>Recent Orders (LLM context)</h2>
  <table>
    <tr><th>Plan</th><th>Provider</th><th>Status</th><th>Paid At</th></tr>
    {order_rows}
  </table>
</div>
</body></html>"#,
        model = config.openrouter_model,
        max = config.pool_max,
        interval = config.pool_interval_secs,
    )
}
