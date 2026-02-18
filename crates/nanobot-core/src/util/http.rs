use once_cell::sync::Lazy;
use reqwest::Client;
use std::time::Duration;

/// Global HTTP client with connection pooling and keep-alive.
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(50)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent("nanobot/0.1.0")
        .build()
        .expect("Failed to create HTTP client")
});

/// Get the global HTTP client.
pub fn client() -> &'static Client {
    &HTTP_CLIENT
}
