# dd-market

Self-service TDX node marketplace. Users pay BTC via BTCPay Server, get a TDX VM with dd-agent assigned to their GitHub org/user.

## Architecture

Rust/axum web app running as a dd-agent workload. Provisions nodes on local baremetal (KVM) first, GCP as overflow.

## Build & Development

```bash
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets
RUSTFLAGS="-Dwarnings" cargo build
```

### Running Locally

```bash
DD_MARKET_PORT=8081 \
BTCPAY_URL=http://localhost:23001 \
BTCPAY_API_KEY=test \
BTCPAY_STORE_ID=test \
BTCPAY_WEBHOOK_SECRET=test \
cargo run
```

## Key Modules

- `config.rs` — env var configuration
- `db.rs` — SQLite schema, migrations, queries
- `models.rs` — Plan, Order, Node, Event types
- `routes.rs` — HTTP handlers
- `btcpay.rs` — BTCPay Greenfield API client
- `provision.rs` — provisioning orchestrator (local first, GCP overflow)
- `gcp.rs` — GCP Compute REST API
- `local.rs` — SSH to baremetal + dd-vm.sh
- `lifecycle.rs` — expiration checks, auto-teardown
- `templates.rs` — inline HTML templates
