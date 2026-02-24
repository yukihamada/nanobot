use colored::*;
use rustyline::Editor;
use std::io::{self, Write};
use std::path::PathBuf;

const DEFAULT_API: &str = "https://chatweb.ai";

struct Cli {
    editor: Editor<()>,
    session_id: String,
    api_base: String,
    auth_token: Option<String>,
    client: reqwest::Client,
}

impl Cli {
    fn new() -> Self {
        let api_base = std::env::var("NANOBOT_API")
            .unwrap_or_else(|_| DEFAULT_API.to_string())
            .trim_end_matches('/')
            .to_string();
        let session_id = std::env::var("NANOBOT_SESSION_ID")
            .unwrap_or_else(|_| get_or_create_session_id());
        let auth_token = std::env::var("NANOBOT_AUTH_TOKEN").ok();
        Self {
            editor: Editor::<()>::new(),
            session_id,
            api_base,
            auth_token,
            client: reqwest::Client::new(),
        }
    }

    async fn send(&self, message: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/api/v1/chat", self.api_base);
        let mut req = self.client.post(&url).json(&serde_json::json!({
            "message": message,
            "session_id": self.session_id,
            "language": "ja",
        }));
        if let Some(token) = &self.auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }
        let resp = req
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("接続エラー: {e}"))?;
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| format!("レスポンス解析エラー: {e}"))
    }

    async fn run(&mut self) {
        println!("{}", "ChatWeb AI".green().bold());
        println!("{}", format!("API: {}", self.api_base).dimmed());
        let id_preview = &self.session_id[..self.session_id.len().min(20)];
        println!("{}", format!("セッション: {}", id_preview).dimmed());
        if self.auth_token.is_some() {
            println!("{}", "認証: 設定済み".dimmed());
        } else {
            println!(
                "{}",
                "ヒント: 管理者機能には NANOBOT_AUTH_TOKEN を設定してください".yellow()
            );
        }
        println!("{}", "終了: /exit または Ctrl+D\n".dimmed());

        loop {
            let prompt = ">> ".blue().to_string();
            match self.editor.readline(&prompt) {
                Ok(line) => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    self.editor.add_history_entry(&line);

                    // Local-only commands (not forwarded to API)
                    match line.as_str() {
                        "/exit" | "/quit" => break,
                        "/clear" => {
                            print!("\x1B[2J\x1B[1;1H");
                            io::stdout().flush().unwrap();
                            continue;
                        }
                        "/help" => {
                            print_help();
                            continue;
                        }
                        _ => {}
                    }

                    // All other input (including /improve, /status, natural language) → API
                    print!("{}", "  思考中...".dimmed());
                    io::stdout().flush().unwrap();

                    match self.send(&line).await {
                        Ok(json) => {
                            // Clear "thinking..." line
                            print!("\r{}\r", " ".repeat(30));
                            io::stdout().flush().unwrap();

                            let response = json["response"].as_str().unwrap_or("(応答なし)");
                            println!("{}", response);

                            // Show tools used
                            if let Some(tools) = json["tools_used"].as_array() {
                                if !tools.is_empty() {
                                    let names: Vec<_> =
                                        tools.iter().filter_map(|t| t.as_str()).collect();
                                    println!(
                                        "{}",
                                        format!("  [ツール: {}]", names.join(", ")).dimmed()
                                    );
                                }
                            }

                            // Show credit usage
                            if let Some(cr) = json["credits_used"].as_i64() {
                                let rem = json["credits_remaining"].as_i64().unwrap_or(-1);
                                if rem >= 0 {
                                    println!(
                                        "{}",
                                        format!("  [-{}cr | 残{}cr]", cr, rem).dimmed()
                                    );
                                } else {
                                    println!("{}", format!("  [-{}cr]", cr).dimmed());
                                }
                            }
                            println!();
                        }
                        Err(e) => {
                            print!("\r{}\r", " ".repeat(30));
                            io::stdout().flush().unwrap();
                            eprintln!("{}", format!("エラー: {}", e).red());
                            println!();
                        }
                    }
                }
                Err(_) => break,
            }
        }

        println!("{}", "Bye!".dimmed());
    }
}

fn print_help() {
    println!("{}", "ローカルコマンド:".cyan().bold());
    println!("  /exit, /quit         終了");
    println!("  /clear               画面クリア");
    println!("  /help                このヘルプ");
    println!();
    println!("{}", "サーバーコマンド（APIに転送）:".cyan().bold());
    println!("  /improve <説明>      自己改善PR作成（管理者のみ）");
    println!("  /improve --confirm <説明>  確認なしで実行");
    println!("  /status              LLMプロバイダー状態");
    println!("  /share               会話を共有");
    println!("  /link [CODE]         チャネル連携");
    println!();
    println!("{}", "自然言語でも動作:".cyan().bold());
    println!("  「このプロジェクトを自己改善してください。」");
    println!("  \"improve this project\"");
    println!();
    println!("{}", "環境変数:".cyan().bold());
    println!("  NANOBOT_API          APIエンドポイント（デフォルト: https://chatweb.ai）");
    println!("  NANOBOT_SESSION_ID   セッションID（管理者メール等を指定可）");
    println!("  NANOBOT_AUTH_TOKEN   Bearer認証トークン");
    println!();
}

fn get_or_create_session_id() -> String {
    let config_dir = home_dir().join(".nanobot");
    let session_file = config_dir.join("session_id");

    if let Ok(id) = std::fs::read_to_string(&session_file) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return format!("cli:{}", id);
        }
    }

    let id = generate_random_id();
    let _ = std::fs::create_dir_all(&config_dir);
    let _ = std::fs::write(&session_file, &id);
    format!("cli:{}", id)
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn generate_random_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    format!("{:08x}{:08x}", nanos, pid)
}

#[tokio::main]
async fn main() {
    let mut cli = Cli::new();
    cli.run().await;
}
