use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct User {
    pub user_id: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub plan: String,
    pub credits_remaining: i64,
    pub credits_used: i64,
}
