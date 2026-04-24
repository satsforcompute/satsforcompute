use anyhow::Result;
use satsforcompute::{config::Config, server};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "satsforcompute=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env()?;
    server::run(cfg).await
}
