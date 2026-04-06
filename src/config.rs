use std::env;

#[derive(Clone)]
pub struct Config {
    pub port: u16,
    pub db_path: String,

    // BTCPay Server
    pub btcpay_url: String,
    pub btcpay_api_key: String,
    pub btcpay_store_id: String,
    pub btcpay_webhook_secret: String,

    // GCP (overflow provisioning)
    pub gcp_project_id: Option<String>,
    pub gcp_zone: String,

    // Local baremetal
    pub baremetal_host: Option<String>,
    pub baremetal_user: String,

    // DD fleet
    pub dd_register_url: String,
    pub dd_binary_url: String,
    pub dd_cf_api_token: Option<String>,
    pub dd_cf_account_id: Option<String>,
    pub dd_cf_zone_id: Option<String>,
    pub dd_cf_domain: String,
    pub dd_github_client_id: Option<String>,
    pub dd_github_client_secret: Option<String>,

    // LLM capacity planner
    pub openrouter_api_key: String,
    pub openrouter_model: String,
    pub pool_max: usize,
    pub pool_interval_secs: u64,

    // Admin
    pub admin_password: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            port: env::var("DD_MARKET_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8081),
            db_path: env::var("DD_MARKET_DB")
                .unwrap_or_else(|_| "/var/lib/dd/shared/marketplace.db".into()),

            btcpay_url: env::var("BTCPAY_URL").unwrap_or_else(|_| "http://localhost:23001".into()),
            btcpay_api_key: env::var("BTCPAY_API_KEY").unwrap_or_default(),
            btcpay_store_id: env::var("BTCPAY_STORE_ID").unwrap_or_default(),
            btcpay_webhook_secret: env::var("BTCPAY_WEBHOOK_SECRET").unwrap_or_default(),

            gcp_project_id: env::var("GCP_PROJECT_ID").ok(),
            gcp_zone: env::var("GCP_ZONE").unwrap_or_else(|_| "us-central1-c".into()),

            baremetal_host: env::var("BAREMETAL_HOST").ok(),
            baremetal_user: env::var("BAREMETAL_USER").unwrap_or_else(|_| "tdx2".into()),

            dd_register_url: env::var("DD_REGISTER_URL")
                .unwrap_or_else(|_| "wss://app.devopsdefender.com/register".into()),
            dd_binary_url: env::var("DD_BINARY_URL").unwrap_or_else(|_| {
                "https://github.com/devopsdefender/dd/releases/latest/download/dd-agent".into()
            }),
            dd_cf_api_token: env::var("DD_CF_API_TOKEN").ok(),
            dd_cf_account_id: env::var("DD_CF_ACCOUNT_ID").ok(),
            dd_cf_zone_id: env::var("DD_CF_ZONE_ID").ok(),
            dd_cf_domain: env::var("DD_CF_DOMAIN").unwrap_or_else(|_| "devopsdefender.com".into()),
            dd_github_client_id: env::var("DD_GITHUB_CLIENT_ID").ok(),
            dd_github_client_secret: env::var("DD_GITHUB_CLIENT_SECRET").ok(),

            openrouter_api_key: env::var("OPENROUTER_API_KEY")
                .expect("OPENROUTER_API_KEY is required"),
            openrouter_model: env::var("OPENROUTER_MODEL")
                .unwrap_or_else(|_| "anthropic/claude-sonnet-4".into()),
            pool_max: env::var("DD_MARKET_POOL_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            pool_interval_secs: env::var("DD_MARKET_POOL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1800),

            admin_password: env::var("DD_MARKET_PASSWORD").ok(),
        }
    }
}
