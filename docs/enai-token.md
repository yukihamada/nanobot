# ENAI Token — Enabler AI Token

発行日: 2026-03-03

## オンチェーン情報

| 項目 | 値 |
|------|-----|
| **Mint Address** | `8CeusiVAeibuBGv5xcf7kt7JQZzqwTS5pD7u2CfyoWnL` |
| **Treasury Wallet** | `DK29rBGCvP83LUNjUGVM6xt6qPy6rycBFopXbFkg9XvQ` |
| **Treasury ATA** | `44RwdzeMf81ePumrNBpjC4optMs3UUf4XDnXJsQKycxq` |
| **Network** | Solana Mainnet-beta |
| **Program** | TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA |
| **Supply** | 1,000,000,000 ENAI |
| **Decimals** | 6 |
| **Metadata URI** | https://chatweb.ai/enai-metadata.json |

## 鍵の保管場所

| 鍵 | 場所 |
|---|---|
| **Keypairファイル (秘密鍵+公開鍵)** | `/Users/yuki/.config/solana/enai-treasury.json` |
| **SOLANA_PRIVATE_KEY** | AWS Lambda 環境変数 (nanobot-prod, ap-northeast-1) |
| **シードフレーズ** | 発行時にターミナルに表示 → 安全な場所に保管必須 |

> ⚠️ `/Users/yuki/.config/solana/enai-treasury.json` を必ずバックアップすること。
> このファイルが失われると treasury の資金にアクセス不可になる。

## 経済設計

- **1 ENAI = 10 credits** (chatweb.ai)
- **1 credit = 0.1 ENAI**
- P2Pノード報酬: 1クエリ処理 → 1 ENAI
- DAO投票: 100 ENAI でプロポーザル投票権

## トークン配分

| 用途 | 割合 | 枚数 |
|------|------|------|
| ユーザー報酬プール (DePIN/コントリビューター) | 30% | 300,000,000 |
| 流動性プール (Raydium/Jupiter) | 20% | 200,000,000 |
| EnablerDAO treasury | 20% | 200,000,000 |
| チーム (2年ベスト) | 15% | 150,000,000 |
| 早期ユーザーエアドロップ | 10% | 100,000,000 |
| 開発費 | 5% | 50,000,000 |

## Lambda 環境変数 (nanobot-prod)

```
ENAI_TOKEN_MINT=8CeusiVAeibuBGv5xcf7kt7JQZzqwTS5pD7u2CfyoWnL
SOLANA_TREASURY_WALLET=DK29rBGCvP83LUNjUGVM6xt6qPy6rycBFopXbFkg9XvQ
SOLANA_PRIVATE_KEY=<base58, Lambda秘密管理>
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
```

## APIエンドポイント

```bash
# レート確認
GET /api/v1/crypto/enai/price

# 支払い開始 (認証必須)
POST /api/v1/crypto/enai/initiate
  { "wallet_address": "<PHANTOM_WALLET>", "credit_amount": 1000 }

# 支払い確認
POST /api/v1/crypto/enai/confirm
  { "tx_id": "<uuid>", "tx_signature": "<SOLANA_SIG>" }

# DePINノード報酬申請
POST /api/v1/depin/report
  { "node_wallet": "<WALLET>", "query_hash": "<HASH>", "proof_timestamp": <unix> }

# DePIN統計
GET /api/v1/depin/stats

# エージェントウォレット残高
GET /api/v1/agent/wallet  (Bearer token必須)
```

## 関連ファイル

- `crates/nanobot-core/src/service/solana.rs` — Solana RPC utilities
- `web/enai-metadata.json` — Metaplex token metadata
- `crates/nanobot-core/src/service/http.rs` — ENAIハンドラー (handle_enai_*)
