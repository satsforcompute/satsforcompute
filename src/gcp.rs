use serde::Deserialize;

use crate::config::Config;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct Operation {
    #[serde(default)]
    #[allow(dead_code)]
    status: String,
    #[serde(default)]
    error: Option<OperationError>,
}

#[derive(Debug, Deserialize)]
struct OperationError {
    errors: Vec<ErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    message: String,
}

pub async fn get_access_token(client: &reqwest::Client) -> Result<String, String> {
    // Try GCP metadata server first (running on GCP VM)
    let resp = client
        .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
        .header("Metadata-Flavor", "Google")
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let token: TokenResponse = r.json().await.map_err(|e| e.to_string())?;
            Ok(token.access_token)
        }
        _ => Err("not running on GCP and no credentials configured".into()),
    }
}

pub fn generate_startup_script(config: &Config, github_handle: &str, vm_name: &str) -> String {
    let register_url = &config.dd_register_url;
    let binary_url = &config.dd_binary_url;
    let cf_api_token = config.dd_cf_api_token.as_deref().unwrap_or("");
    let cf_account_id = config.dd_cf_account_id.as_deref().unwrap_or("");
    let cf_zone_id = config.dd_cf_zone_id.as_deref().unwrap_or("");
    let cf_domain = &config.dd_cf_domain;
    let gh_client_id = config.dd_github_client_id.as_deref().unwrap_or("");
    let gh_client_secret = config.dd_github_client_secret.as_deref().unwrap_or("");

    format!(
        r#"#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get update -q
apt-get install -y podman
systemctl enable --now podman.socket

curl -fsSL -o /usr/local/bin/cloudflared \
  https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64
chmod +x /usr/local/bin/cloudflared

curl -fsSL -o /usr/local/bin/dd-agent "{binary_url}" -H "Accept: application/octet-stream"
chmod +x /usr/local/bin/dd-agent

DD_OWNER="{github_handle}" \
DD_AGENT_MODE=agent \
DD_ENV=production \
DD_REGISTER_URL="{register_url}" \
DD_CF_API_TOKEN="{cf_api_token}" \
DD_CF_ACCOUNT_ID="{cf_account_id}" \
DD_CF_ZONE_ID="{cf_zone_id}" \
DD_CF_DOMAIN="{cf_domain}" \
DD_GITHUB_CLIENT_ID="{gh_client_id}" \
DD_GITHUB_CLIENT_SECRET="{gh_client_secret}" \
DD_GITHUB_CALLBACK_URL="https://{vm_name}.{cf_domain}/auth/github/callback" \
DD_BOOT_CMD=bash \
DD_BOOT_APP=shell \
nohup /usr/local/bin/dd-agent > /var/log/dd-agent.log 2>&1 &
"#
    )
}

pub async fn create_instance(
    client: &reqwest::Client,
    config: &Config,
    vm_name: &str,
    machine_type: &str,
    disk_gb: i64,
    startup_script: &str,
    github_handle: &str,
) -> Result<(), String> {
    let project = config
        .gcp_project_id
        .as_deref()
        .ok_or("GCP_PROJECT_ID not configured")?;
    let zone = &config.gcp_zone;
    let token = get_access_token(client).await?;

    let body = serde_json::json!({
        "name": vm_name,
        "machineType": format!("zones/{zone}/machineTypes/{machine_type}"),
        "confidentialInstanceConfig": {
            "confidentialInstanceType": "TDX"
        },
        "scheduling": {
            "onHostMaintenance": "TERMINATE"
        },
        "disks": [{
            "boot": true,
            "autoDelete": true,
            "initializeParams": {
                "sourceImage": "projects/ubuntu-os-cloud/global/images/family/ubuntu-2404-lts-amd64",
                "diskSizeGb": disk_gb.to_string()
            }
        }],
        "networkInterfaces": [{
            "accessConfigs": [{"type": "ONE_TO_ONE_NAT", "name": "External NAT"}]
        }],
        "metadata": {
            "items": [{
                "key": "startup-script",
                "value": startup_script
            }]
        },
        "labels": {
            "devopsdefender": "managed",
            "dd_source": "marketplace",
            "dd_owner": github_handle.to_lowercase()
        },
        "tags": {
            "items": ["dd-agent"]
        }
    });

    let resp = client
        .post(format!(
            "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/instances"
        ))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("gcp create failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("gcp returned {status}: {text}"));
    }

    let op: Operation = resp.json().await.map_err(|e| format!("gcp parse: {e}"))?;
    if let Some(err) = op.error {
        let msgs: Vec<_> = err.errors.iter().map(|e| e.message.as_str()).collect();
        return Err(format!("gcp error: {}", msgs.join(", ")));
    }

    Ok(())
}

pub async fn delete_instance(
    client: &reqwest::Client,
    config: &Config,
    vm_name: &str,
) -> Result<(), String> {
    let project = config
        .gcp_project_id
        .as_deref()
        .ok_or("GCP_PROJECT_ID not configured")?;
    let zone = &config.gcp_zone;
    let token = get_access_token(client).await?;

    let resp = client
        .delete(format!(
            "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/instances/{vm_name}"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("gcp delete failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("gcp delete returned {status}: {text}"));
    }

    Ok(())
}
