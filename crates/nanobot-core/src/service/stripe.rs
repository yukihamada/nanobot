use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::service::auth::Plan;

/// Stripe webhook event types we handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripeEventType {
    CheckoutSessionCompleted,
    CustomerSubscriptionUpdated,
    CustomerSubscriptionDeleted,
    InvoicePaid,
    InvoicePaymentFailed,
    Unknown(String),
}

impl From<&str> for StripeEventType {
    fn from(s: &str) -> Self {
        match s {
            "checkout.session.completed" => Self::CheckoutSessionCompleted,
            "customer.subscription.updated" => Self::CustomerSubscriptionUpdated,
            "customer.subscription.deleted" => Self::CustomerSubscriptionDeleted,
            "invoice.paid" => Self::InvoicePaid,
            "invoice.payment_failed" => Self::InvoicePaymentFailed,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// Result of processing a Stripe webhook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookResult {
    pub event_type: String,
    pub action: String,
    pub tenant_id: Option<String>,
}

/// Map a Stripe price ID to a Plan.
pub fn price_to_plan(price_id: &str) -> Option<Plan> {
    // These would be configured per environment
    if price_id.contains("starter") {
        Some(Plan::Starter)
    } else if price_id.contains("pro") {
        Some(Plan::Pro)
    } else if price_id.contains("enterprise") {
        Some(Plan::Enterprise)
    } else {
        None
    }
}

/// Verify a Stripe webhook signature.
///
/// In production, use the async-stripe crate's webhook verification.
/// This is a simplified version.
#[cfg(feature = "http-api")]
pub fn verify_webhook_signature(
    payload: &[u8],
    signature: &str,
    webhook_secret: &str,
) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // Parse Stripe signature format: t=timestamp,v1=signature
    let parts: std::collections::HashMap<&str, &str> = signature
        .split(',')
        .filter_map(|part| part.split_once('='))
        .collect();

    let timestamp = match parts.get("t") {
        Some(t) => *t,
        None => return false,
    };

    let expected_sig = match parts.get("v1") {
        Some(s) => *s,
        None => return false,
    };

    // Construct signed payload
    let signed_payload = format!("{}.{}", timestamp, String::from_utf8_lossy(payload));

    type HmacSha256 = Hmac<Sha256>;
    let Ok(mut mac) = HmacSha256::new_from_slice(webhook_secret.as_bytes()) else {
        return false;
    };
    mac.update(signed_payload.as_bytes());

    let computed = hex::encode(mac.finalize().into_bytes());
    computed == expected_sig
}

/// Process a Stripe webhook event (simplified).
pub fn process_webhook_event(event_type: &str, event_json: &serde_json::Value) -> WebhookResult {
    let event = StripeEventType::from(event_type);
    info!("Processing Stripe event: {:?}", event);

    match event {
        StripeEventType::CheckoutSessionCompleted => {
            let customer_id = event_json
                .pointer("/data/object/customer")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            info!(
                "Checkout completed for customer: {}",
                customer_id
            );

            WebhookResult {
                event_type: event_type.to_string(),
                action: "tenant_created".to_string(),
                tenant_id: Some(customer_id.to_string()),
            }
        }
        StripeEventType::CustomerSubscriptionUpdated => {
            let customer_id = event_json
                .pointer("/data/object/customer")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            info!("Subscription updated for customer: {}", customer_id);

            WebhookResult {
                event_type: event_type.to_string(),
                action: "plan_changed".to_string(),
                tenant_id: Some(customer_id.to_string()),
            }
        }
        StripeEventType::CustomerSubscriptionDeleted => {
            let customer_id = event_json
                .pointer("/data/object/customer")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            info!("Subscription deleted for customer: {}", customer_id);

            WebhookResult {
                event_type: event_type.to_string(),
                action: "downgraded_to_free".to_string(),
                tenant_id: Some(customer_id.to_string()),
            }
        }
        StripeEventType::InvoicePaid => {
            let customer_id = event_json
                .pointer("/data/object/customer")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            info!("Invoice paid for customer: {}", customer_id);

            WebhookResult {
                event_type: event_type.to_string(),
                action: "credits_reset".to_string(),
                tenant_id: Some(customer_id.to_string()),
            }
        }
        StripeEventType::InvoicePaymentFailed => {
            let customer_id = event_json
                .pointer("/data/object/customer")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            warn!("Payment failed for customer: {}", customer_id);

            WebhookResult {
                event_type: event_type.to_string(),
                action: "payment_failed".to_string(),
                tenant_id: Some(customer_id.to_string()),
            }
        }
        StripeEventType::Unknown(ref t) => {
            info!("Ignoring unknown Stripe event: {}", t);
            WebhookResult {
                event_type: event_type.to_string(),
                action: "ignored".to_string(),
                tenant_id: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stripe_event_type_from_str() {
        assert_eq!(
            StripeEventType::from("checkout.session.completed"),
            StripeEventType::CheckoutSessionCompleted
        );
        assert_eq!(
            StripeEventType::from("invoice.paid"),
            StripeEventType::InvoicePaid
        );
        assert!(matches!(
            StripeEventType::from("unknown.event"),
            StripeEventType::Unknown(_)
        ));
    }

    #[test]
    fn test_price_to_plan() {
        assert_eq!(price_to_plan("price_starter_monthly"), Some(Plan::Starter));
        assert_eq!(price_to_plan("price_pro_monthly"), Some(Plan::Pro));
        assert_eq!(price_to_plan("price_enterprise_annual"), Some(Plan::Enterprise));
        assert_eq!(price_to_plan("price_unknown"), None);
    }

    #[test]
    fn test_process_checkout_completed() {
        let event = serde_json::json!({
            "data": {
                "object": {
                    "customer": "cus_abc123"
                }
            }
        });

        let result = process_webhook_event("checkout.session.completed", &event);
        assert_eq!(result.action, "tenant_created");
        assert_eq!(result.tenant_id.as_deref(), Some("cus_abc123"));
    }

    #[test]
    fn test_process_subscription_deleted() {
        let event = serde_json::json!({
            "data": {
                "object": {
                    "customer": "cus_xyz789"
                }
            }
        });

        let result = process_webhook_event("customer.subscription.deleted", &event);
        assert_eq!(result.action, "downgraded_to_free");
    }

    #[test]
    fn test_process_unknown_event() {
        let event = serde_json::json!({});
        let result = process_webhook_event("unknown.event", &event);
        assert_eq!(result.action, "ignored");
    }
}
