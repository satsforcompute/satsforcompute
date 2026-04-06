use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub machine_type: String,
    pub vcpus: i64,
    pub ram_gb: i64,
    pub disk_gb: i64,
    pub duration_hours: i64,
    pub price_sats: i64,
    pub provider: Provider,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Local,
    Gcp,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Gcp => "gcp",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "gcp" => Provider::Gcp,
            _ => Provider::Local,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    PendingPayment,
    Paid,
    Provisioning,
    Active,
    Expired,
    Failed,
}

impl OrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderStatus::PendingPayment => "pending_payment",
            OrderStatus::Paid => "paid",
            OrderStatus::Provisioning => "provisioning",
            OrderStatus::Active => "active",
            OrderStatus::Expired => "expired",
            OrderStatus::Failed => "failed",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "pending_payment" => OrderStatus::PendingPayment,
            "paid" => OrderStatus::Paid,
            "provisioning" => OrderStatus::Provisioning,
            "active" => OrderStatus::Active,
            "expired" => OrderStatus::Expired,
            "failed" => OrderStatus::Failed,
            _ => OrderStatus::Failed,
        }
    }

    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            OrderStatus::PendingPayment => "Pending Payment",
            OrderStatus::Paid => "Paid",
            OrderStatus::Provisioning => "Provisioning",
            OrderStatus::Active => "Active",
            OrderStatus::Expired => "Expired",
            OrderStatus::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub github_handle: String,
    pub plan_id: String,
    pub status: OrderStatus,
    pub btcpay_invoice_id: Option<String>,
    pub price_sats: i64,
    pub created_at: String,
    pub paid_at: Option<String>,
    pub provisioned_at: Option<String>,
    pub expires_at: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Warm,
    Provisioning,
    Running,
    Stopped,
    Deleted,
}

impl NodeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeStatus::Warm => "warm",
            NodeStatus::Provisioning => "provisioning",
            NodeStatus::Running => "running",
            NodeStatus::Stopped => "stopped",
            NodeStatus::Deleted => "deleted",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "warm" => NodeStatus::Warm,
            "provisioning" => NodeStatus::Provisioning,
            "running" => NodeStatus::Running,
            "stopped" => NodeStatus::Stopped,
            "deleted" => NodeStatus::Deleted,
            _ => NodeStatus::Deleted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub order_id: String,
    pub github_handle: String,
    pub provider: Provider,
    pub vm_name: String,
    pub hostname: Option<String>,
    pub status: NodeStatus,
    pub created_at: String,
    pub expires_at: String,
}
