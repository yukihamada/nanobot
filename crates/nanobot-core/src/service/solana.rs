//! Solana ENAI token utilities.
//! Uses Solana JSON-RPC via reqwest (no solana-sdk dependency needed).
//! Supports both legacy SPL Token and Token-2022 programs.

use anyhow::{Result, bail};
use serde_json::Value;

/// Token-2022 program ID.
const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
/// Legacy SPL Token program ID.
const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

/// ENAI token mint address (set via env ENAI_TOKEN_MINT).
/// For ENAI v2 (Token-2022), set this to the new mint address.
pub fn enai_mint() -> String {
    std::env::var("ENAI_TOKEN_MINT")
        .unwrap_or_else(|_| "ENAIxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".to_string())
}

/// Whether the current ENAI mint is a Token-2022 token.
/// Set ENAI_TOKEN_PROGRAM=token-2022 to enable Token-2022 queries.
pub fn is_token_2022() -> bool {
    std::env::var("ENAI_TOKEN_PROGRAM")
        .map(|v| v == "token-2022" || v == "Token-2022")
        .unwrap_or(false)
}

/// Returns the appropriate token program ID for the current ENAI configuration.
pub fn token_program_id() -> &'static str {
    if is_token_2022() {
        TOKEN_2022_PROGRAM
    } else {
        TOKEN_PROGRAM
    }
}

/// Solana RPC URL (mainnet-beta or devnet).
pub fn rpc_url() -> String {
    std::env::var("SOLANA_RPC_URL")
        .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string())
}

/// Treasury wallet public key.
pub fn treasury_wallet() -> String {
    std::env::var("SOLANA_TREASURY_WALLET").unwrap_or_default()
}

/// Get ENAI token balance for a wallet address.
/// Returns the balance in raw units (divided by 10^6 = actual ENAI amount).
/// Queries the correct token program (SPL Token or Token-2022) based on config.
pub async fn get_enai_balance(wallet_address: &str) -> Result<u64> {
    let mint = enai_mint();
    if mint.starts_with("ENAI") {
        // placeholder mint
        return Ok(0);
    }

    let program = token_program_id();
    let client = reqwest::Client::new();
    let resp = client
        .post(&rpc_url())
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountsByOwner",
            "params": [
                wallet_address,
                { "mint": mint, "programId": program },
                { "encoding": "jsonParsed" }
            ]
        }))
        .send()
        .await?
        .json::<Value>()
        .await?;

    let amount = resp
        .pointer("/result/value/0/account/data/parsed/info/tokenAmount/amount")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok(amount)
}

/// Verify that a transaction signature transferred `required_enai_raw` (raw units) of ENAI
/// to the treasury. Returns the actual amount transferred on success.
pub async fn verify_enai_payment(tx_signature: &str, required_enai_raw: u64) -> Result<u64> {
    let treasury = treasury_wallet();
    if treasury.is_empty() {
        bail!("SOLANA_TREASURY_WALLET not configured");
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&rpc_url())
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTransaction",
            "params": [
                tx_signature,
                {
                    "encoding": "jsonParsed",
                    "maxSupportedTransactionVersion": 0,
                    "commitment": "finalized"
                }
            ]
        }))
        .send()
        .await?
        .json::<Value>()
        .await?;

    // Check transaction exists
    let result = resp
        .get("result")
        .and_then(|v| if v.is_null() { None } else { Some(v) });
    let result = match result {
        Some(r) => r,
        None => bail!("Transaction not found or not finalized"),
    };

    // Check transaction succeeded (no error)
    if !result
        .pointer("/meta/err")
        .map(|v| v.is_null())
        .unwrap_or(false)
    {
        bail!("Transaction failed on-chain");
    }

    let mint = enai_mint();

    // Check postTokenBalances for treasury receiving ENAI
    let post_balances = result
        .pointer("/meta/postTokenBalances")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let pre_balances = result
        .pointer("/meta/preTokenBalances")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Find treasury's token account index and compute delta
    let account_keys: Vec<&str> = result
        .pointer("/transaction/message/accountKeys")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|k| k.get("pubkey").and_then(|p| p.as_str()))
                .collect()
        })
        .unwrap_or_default();

    // Find pre/post amount for the treasury token account
    let mut received: u64 = 0;
    for post in &post_balances {
        let post_mint = post
            .pointer("/mint")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if post_mint != mint {
            continue;
        }

        let account_idx = post
            .get("accountIndex")
            .and_then(|v| v.as_u64())
            .unwrap_or(999) as usize;
        let owner = post
            .pointer("/owner")
            .and_then(|v| v.as_str())
            .or_else(|| account_keys.get(account_idx).copied())
            .unwrap_or("");

        // Check if this token account is owned by treasury
        if owner != treasury {
            continue;
        }

        let post_amount: u64 = post
            .pointer("/uiTokenAmount/amount")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let pre_amount: u64 = pre_balances
            .iter()
            .find(|pre| {
                pre.get("accountIndex") == post.get("accountIndex")
                    && pre
                        .pointer("/mint")
                        .and_then(|v| v.as_str())
                        .map(|m| m == mint.as_str())
                        .unwrap_or(false)
            })
            .and_then(|pre| {
                pre.pointer("/uiTokenAmount/amount")
                    .and_then(|v| v.as_str())
            })
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if post_amount > pre_amount {
            received = post_amount - pre_amount;
            break;
        }
    }

    if received < required_enai_raw {
        bail!(
            "Insufficient ENAI sent: got {} raw units, need {}",
            received,
            required_enai_raw
        );
    }

    Ok(received)
}

/// Convert credits to raw ENAI units (1 ENAI = 10 credits, 6 decimals → 1 ENAI = 1_000_000 raw)
pub fn credits_to_enai_raw(credits: i64) -> u64 {
    // 1 credit = 0.1 ENAI = 100_000 raw units
    (credits as u64) * 100_000
}

/// Convert raw ENAI units to display amount (divide by 10^6)
pub fn raw_to_enai_display(raw: u64) -> f64 {
    raw as f64 / 1_000_000.0
}
