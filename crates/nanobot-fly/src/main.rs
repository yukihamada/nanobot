use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::response::{Html, IntoResponse, Response};
use tracing::info;

use nanobot_core::config;
use nanobot_core::db::{DbBackend, LibSqlBackend};
use nanobot_core::service::http::{create_router, AppState};
use nanobot_core::session::file_store::FileSessionStore;

/// Known API key env var names. On startup we load these from the DB config store
/// (same key pattern as DynamoDB: pk="CONFIG#api_keys", sk="LATEST").
const API_KEY_NAMES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "OPENROUTER_API_KEY",
    "MINIMAX_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "ELEVENLABS_API_KEY",
    "REPLICATE_API_TOKEN",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ---------------------------------------------------------------------------
    // Logging
    // ---------------------------------------------------------------------------
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("nanobot=info".parse().unwrap()),
        )
        .init();

    info!("nanobot-fly starting (libSQL backend)…");

    // ---------------------------------------------------------------------------
    // Database — selected by DATABASE_URL env var
    //   libsql://xxx.turso.io  → Turso cloud
    //   /data/nanobot.db       → Fly.io volume (SQLite file)
    //   :memory:               → ephemeral (tests / dev)
    // ---------------------------------------------------------------------------
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "/data/nanobot.db".to_string());
    let db_token = std::env::var("DATABASE_TOKEN").ok();

    let db = LibSqlBackend::new(&db_url, db_token.as_deref())
        .await
        .context("Failed to initialise database backend")?;

    // Run schema migrations (idempotent CREATE TABLE IF NOT EXISTS)
    db.run_migrations()
        .await
        .context("Database migrations failed")?;

    info!("Database ready: {}", db_url);

    // ---------------------------------------------------------------------------
    // Load API keys from DB config store (same pattern as Lambda/DynamoDB).
    // Values in the DB override environment variables so the admin panel works.
    // ---------------------------------------------------------------------------
    if let Ok(Some(serde_json::Value::Object(map))) = db
        .get_config("CONFIG#api_keys", "LATEST")
        .await
    {
        let mut loaded = 0u32;
        for &name in API_KEY_NAMES {
            if let Some(serde_json::Value::String(val)) = map.get(name) {
                if !val.is_empty() {
                    // SAFETY: single-threaded startup, no concurrent readers yet.
                    unsafe { std::env::set_var(name, val) };
                    loaded += 1;
                }
            }
        }
        info!("Loaded {} API keys from DB config store", loaded);
    } else {
        info!("No CONFIG#api_keys in DB, using env vars only");
    }

    // ---------------------------------------------------------------------------
    // Session store — file-based (persistent across restarts via Fly volume)
    // ---------------------------------------------------------------------------
    let data_dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "/data".to_string());
    let data_path = std::path::Path::new(&data_dir);
    std::fs::create_dir_all(data_path).ok();
    let session_store = FileSessionStore::new(data_path);

    // ---------------------------------------------------------------------------
    // AppState
    // ---------------------------------------------------------------------------
    let cfg = config::load_config_from_env();
    let mut app_state = AppState::with_provider(cfg, Box::new(session_store));
    app_state.db = Some(Arc::new(db));

    // Load MCP tools from environment
    let mcp_tools = nanobot_core::mcp::client::load_mcp_tools_from_env().await;
    if !mcp_tools.is_empty() {
        info!("Loaded {} MCP tools", mcp_tools.len());
        app_state.tool_registry.register_all(mcp_tools);
    }
    info!("Total tools registered: {}", app_state.tool_registry.len());

    let state = Arc::new(app_state);
    let mut router = create_router(state);

    // ---------------------------------------------------------------------------
    // Static file serving — WASM SPA replaces legacy HTML
    // ---------------------------------------------------------------------------
    let static_dir = PathBuf::from(
        std::env::var("STATIC_DIR").unwrap_or_else(|_| "./static".to_string()),
    );

    if static_dir.exists() {
        info!("Serving static files from {}", static_dir.display());

        // Serve /pkg/* for WASM bundles, JS, CSS
        let serve_dir = tower_http::services::ServeDir::new(&static_dir)
            .append_index_html_on_directories(false);
        router = router.nest_service("/pkg", serve_dir);

        // SPA fallback: WASM index.html for all unmatched GET requests.
        // This covers /, /app, /app/*, and any other non-API page route.
        let spa_index = static_dir.join("index.html");
        if spa_index.exists() {
            let index_html = std::fs::read_to_string(&spa_index)
                .expect("Failed to read WASM index.html");
            let index_html = Arc::new(index_html);

            router = router.fallback(move |req: axum::extract::Request| {
                let html = index_html.clone();
                async move {
                    // Only serve SPA for GET requests (POST/PUT etc. get 404)
                    if req.method() == axum::http::Method::GET {
                        let mut resp: Response = Html(html.to_string()).into_response();
                        resp.headers_mut().insert(
                            axum::http::header::CONTENT_SECURITY_POLICY,
                            "default-src 'self'; \
                             script-src 'self' 'unsafe-inline' 'unsafe-eval' 'wasm-unsafe-eval' https://cdn.tailwindcss.com; \
                             style-src 'self' 'unsafe-inline'; \
                             img-src 'self' data: blob: https: http:; \
                             connect-src 'self' https: wss:; \
                             object-src 'none'; \
                             base-uri 'self'"
                                .parse()
                                .unwrap(),
                        );
                        resp
                    } else {
                        axum::http::StatusCode::NOT_FOUND.into_response()
                    }
                }
            });

            info!("WASM SPA fallback enabled for all unmatched GET routes");
        }
    } else {
        info!("No static dir at {} — WASM app disabled", static_dir.display());
    }

    // ---------------------------------------------------------------------------
    // HTTP server
    // ---------------------------------------------------------------------------
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("nanobot-fly listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("Failed to bind TCP listener")?;

    axum::serve(listener, router)
        .await
        .context("HTTP server error")?;

    Ok(())
}
