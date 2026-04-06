use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::Config;

#[derive(Debug, Serialize)]
struct CreateInvoiceRequest {
    amount: String,
    currency: String,
    metadata: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkout: Option<CheckoutOptions>,
}

#[derive(Debug, Serialize)]
struct CheckoutOptions {
    #[serde(rename = "expirationMinutes")]
    expiration_minutes: i64,
}

#[derive(Debug, Deserialize)]
pub struct InvoiceResponse {
    pub id: String,
    #[serde(rename = "checkoutLink")]
    #[allow(dead_code)]
    pub checkout_link: Option<String>,
    #[allow(dead_code)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WebhookPayload {
    #[serde(rename = "invoiceId")]
    pub invoice_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
}

pub struct BtcPayClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    store_id: String,
    webhook_secret: String,
}

impl BtcPayClient {
    pub fn new(config: &Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: config.btcpay_url.trim_end_matches('/').to_string(),
            api_key: config.btcpay_api_key.clone(),
            store_id: config.btcpay_store_id.clone(),
            webhook_secret: config.btcpay_webhook_secret.clone(),
        }
    }

    pub async fn create_invoice(
        &self,
        amount_sats: i64,
        order_id: &str,
        github_handle: &str,
    ) -> Result<InvoiceResponse, String> {
        let btc_amount = amount_sats as f64 / 100_000_000.0;
        let body = CreateInvoiceRequest {
            amount: format!("{btc_amount:.8}"),
            currency: "BTC".into(),
            metadata: serde_json::json!({
                "orderId": order_id,
                "githubHandle": github_handle,
            }),
            checkout: Some(CheckoutOptions {
                expiration_minutes: 60,
            }),
        };

        let resp = self
            .client
            .post(format!(
                "{}/api/v1/stores/{}/invoices",
                self.base_url, self.store_id
            ))
            .header("Authorization", format!("token {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("btcpay request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("btcpay returned {status}: {text}"));
        }

        resp.json::<InvoiceResponse>()
            .await
            .map_err(|e| format!("btcpay parse failed: {e}"))
    }

    pub fn verify_webhook(&self, body: &[u8], signature: &str) -> bool {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.webhook_secret.as_bytes()).expect("hmac key");
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        // BTCPay sends "sha256=HEXDIGEST"
        let sig = signature.strip_prefix("sha256=").unwrap_or(signature);
        constant_time_eq(sig.as_bytes(), expected.as_bytes())
    }

    pub fn checkout_url(&self, invoice_id: &str) -> String {
        format!("{}/i/{}", self.base_url, invoice_id)
    }
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

pub fn parse_webhook(body: &[u8]) -> Result<WebhookPayload, String> {
    serde_json::from_slice(body).map_err(|e| format!("webhook parse failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_webhook_signature() {
        let client = BtcPayClient {
            client: reqwest::Client::new(),
            base_url: String::new(),
            api_key: String::new(),
            store_id: String::new(),
            webhook_secret: "testsecret".into(),
        };
        let body = b"test body";
        let mut mac = Hmac::<Sha256>::new_from_slice(b"testsecret").unwrap();
        mac.update(body);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(client.verify_webhook(body, &sig));
        assert!(!client.verify_webhook(body, "sha256=0000"));
    }

    #[test]
    fn test_parse_webhook() {
        let body = br#"{"invoiceId":"abc123","type":"InvoiceSettled"}"#;
        let payload = parse_webhook(body).unwrap();
        assert_eq!(payload.invoice_id, "abc123");
        assert_eq!(payload.event_type, "InvoiceSettled");
    }
}
