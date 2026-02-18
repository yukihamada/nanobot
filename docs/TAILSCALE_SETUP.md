# 🔐 Tailscale + Bearer Token + IP制限セットアップガイド

**最もセキュアな分散推論の設定方法**

## 🎯 セキュリティレベル

- 🔒 **E2E暗号化**: WireGuard プロトコル（軍事レベル）
- 🔒 **Bearer Token認証**: UUIDトークンによるアクセス制御
- 🔒 **IP制限**: Tailscaleプライベートネット（100.64.0.0/10）のみ許可
- ✅ **NAT越え**: 自動でファイアウォール・NATを通過
- ✅ **ゼロコンフィグ**: VPN設定不要、簡単セットアップ

---

## 📋 前提条件

- macOS / Linux / Windows
- インターネット接続
- cargo（Rustビルド環境）
- sudo権限（Tailscaleインストール時のみ）

---

## 🚀 セットアップ手順

### ステップ1: サーバー側セットアップ（推論を提供するPC）

```bash
# リポジトリに移動
cd /Users/yuki/workspace/nanobot

# 自動セットアップスクリプトを実行
./scripts/setup-tailscale.sh
```

**スクリプトの実行内容:**
1. Tailscaleのインストール確認（未インストールなら自動インストール）
2. Tailscaleの起動（ブラウザで認証）
3. Tailscale IPアドレスの取得
4. APIトークンの生成（または既存トークンの確認）
5. config.jsonの自動設定

**出力例:**
```
🔐 Nanobot Secure Gateway Setup (Tailscale + Token + IP)
=========================================================

✅ Tailscale installed: 1.94.1
📍 Your Tailscale IP: 100.64.1.5
🔑 Generating new API token...
   Token: a34704a8-9c52-48d1-8b5c-f0ac6045ca18
   ✅ Token saved to config.json

📋 Current Gateway Configuration:
{
  "host": "0.0.0.0",
  "port": 3000,
  "apiTokens": [
    "a34704a8-9c52-48d1-8b5c-f0ac6045ca18"
  ],
  "allowedIps": [
    "127.0.0.1",
    "100.64.0.0/10"
  ],
  "tlsCert": null,
  "tlsKey": null
}

✅ Setup complete!
```

### ステップ2: Gatewayサーバーを起動

```bash
# リリースビルド（本番用）
cargo build --features http-api --release
./target/release/chatweb gateway --http --http-port 3000 --auth

# または開発ビルド（テスト用）
cargo build --features http-api
./target/debug/chatweb gateway --http --http-port 3000 --auth
```

**起動ログ例:**
```
🐈 Starting chatweb HTTP API on 0.0.0.0:3000...
  Authentication: ENABLED
[INFO] Gateway IP restriction enabled (2 entries)
[INFO] Gateway authentication enabled (1 tokens configured)
[INFO] HTTP server listening on 0.0.0.0:3000
[INFO] nanobot gateway started
```

### ステップ3: クライアント側セットアップ（推論を利用するPC）

```bash
# 別のPC（ラップトップ、リモートサーバーなど）で実行

# 1. Tailscaleをインストール＆起動
brew install tailscale  # macOS
# または
curl -fsSL https://tailscale.com/install.sh | sh  # Linux

sudo tailscale up

# 2. サーバーのTailscale IPを確認（サーバー側で実行）
tailscale ip -4
# 出力例: 100.64.1.5

# 3. 接続テストスクリプトを実行
cd /Users/yuki/workspace/nanobot
./scripts/connect-tailscale.sh 100.64.1.5
```

**接続テストの出力例:**
```
🔗 Nanobot Tailscale Client Connection
=======================================

📍 Your Tailscale IP: 100.64.1.10
🎯 Server IP: 100.64.1.5

🔑 Enter API token (or press Enter to use default from config.json):
✅ Using token: a34704a8...045ca18

🧪 Testing connection...
✅ Connection successful!

Response:
Hello from Tailscale! I'm ready to help you.

🎉 You can now use the nanobot gateway securely via Tailscale!
```

---

## 📝 手動設定（スクリプトを使わない場合）

### サーバー側: config.json

```json
{
  "gateway": {
    "host": "0.0.0.0",
    "port": 3000,
    "apiTokens": [
      "your-secure-token-here"
    ],
    "allowedIps": [
      "127.0.0.1",       // localhost
      "100.64.0.0/10"    // Tailscale プライベートネット全体
    ],
    "tlsCert": null,
    "tlsKey": null
  }
}
```

### クライアント側: APIリクエスト

```bash
# 環境変数設定
export NANOBOT_SERVER="http://100.64.1.5:3000"
export NANOBOT_TOKEN="a34704a8-9c52-48d1-8b5c-f0ac6045ca18"

# チャットリクエスト
curl -H "Authorization: Bearer $NANOBOT_TOKEN" \
  $NANOBOT_SERVER/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{
    "message": "こんにちは！",
    "session_id": "my-session"
  }'

# ストリーミング（SSE）
curl -N -H "Authorization: Bearer $NANOBOT_TOKEN" \
  $NANOBOT_SERVER/api/v1/chat/stream \
  -H "Content-Type: application/json" \
  -d '{
    "message": "長い文章を生成して",
    "session_id": "my-session"
  }'

# ヘルスチェック（認証不要）
curl $NANOBOT_SERVER/health
```

---

## 🔧 トラブルシューティング

### 1. Tailscaleに接続できない

```bash
# Tailscaleのステータス確認
tailscale status

# Tailscaleを再起動
sudo tailscale down
sudo tailscale up

# ネットワーク疎通確認
tailscale ping <server-ip>
```

### 2. 認証エラー (401 Unauthorized)

**原因:** APIトークンが間違っている

**解決方法:**
```bash
# サーバー側のトークン確認
jq -r '.gateway.apiTokens[]' ~/.nanobot/config.json

# 正しいトークンをクライアント側で設定
export NANOBOT_TOKEN="correct-token-here"
```

### 3. IP制限エラー (403 Forbidden)

**原因:** クライアントのIPがallowedIpsに含まれていない

**解決方法:**
```bash
# クライアントのTailscale IP確認
tailscale ip -4
# 出力例: 100.64.1.10

# サーバー側のconfig.jsonを確認
jq '.gateway.allowedIps' ~/.nanobot/config.json
# "100.64.0.0/10" が含まれていることを確認

# Tailscale IPは必ず 100.64.x.x の範囲
```

### 4. サーバーが起動しない

```bash
# ポートが使用中か確認
lsof -i :3000

# 別のポートで起動
./target/release/chatweb gateway --http --http-port 8080 --auth
```

---

## 🎯 実践例

### 例1: ノートPCから自宅PCの推論を利用

```bash
# 自宅PC（Mac mini, Tailscale IP: 100.64.1.5）
./target/release/chatweb gateway --http --http-port 3000 --auth

# 外出先ノートPC（MacBook）
export NANOBOT_SERVER="http://100.64.1.5:3000"
export NANOBOT_TOKEN="a34704a8-9c52-48d1-8b5c-f0ac6045ca18"

curl -H "Authorization: Bearer $NANOBOT_TOKEN" \
  $NANOBOT_SERVER/api/v1/chat \
  -d '{"message": "画像解析して", "session_id": "macbook"}'
```

### 例2: VPSから自宅GPUサーバーの推論を利用

```bash
# 自宅GPUサーバー（RTX 4090, Tailscale IP: 100.64.1.8）
ANTHROPIC_API_KEY=sk-ant-xxx \
./target/release/chatweb gateway --http --http-port 3000 --auth

# VPS（クラウドサーバー）
export NANOBOT_SERVER="http://100.64.1.8:3000"
export NANOBOT_TOKEN="your-token"

# APIサーバーとして利用
curl -H "Authorization: Bearer $NANOBOT_TOKEN" \
  $NANOBOT_SERVER/api/v1/chat \
  -d '{"message": "大規模データ分析", "session_id": "vps-worker"}'
```

### 例3: チーム内で推論リソース共有

```bash
# チームの共有サーバー（Tailscale IP: 100.64.1.20）
# 複数のAPIトークンを発行
{
  "gateway": {
    "apiTokens": [
      "token-for-alice",
      "token-for-bob",
      "token-for-charlie"
    ],
    "allowedIps": ["100.64.0.0/10"]
  }
}

# メンバーAlice
export NANOBOT_TOKEN="token-for-alice"
curl -H "Authorization: Bearer $NANOBOT_TOKEN" \
  http://100.64.1.20:3000/api/v1/chat \
  -d '{"message": "プロジェクト分析", "session_id": "alice"}'

# メンバーBob
export NANOBOT_TOKEN="token-for-bob"
curl -H "Authorization: Bearer $NANOBOT_TOKEN" \
  http://100.64.1.20:3000/api/v1/chat \
  -d '{"message": "コードレビュー", "session_id": "bob"}'
```

---

## 📊 セキュリティチェックリスト

- [ ] Tailscaleが正常に動作している (`tailscale status`)
- [ ] APIトークンが強力（UUIDまたは32文字以上のランダム文字列）
- [ ] config.jsonのallowedIpsに`100.64.0.0/10`が含まれている
- [ ] Tailscale以外のネットワークからアクセスできないことを確認
- [ ] APIトークンを安全に管理（環境変数、シークレット管理ツール）
- [ ] 定期的にTailscaleを更新（セキュリティパッチ）

---

## 🚀 次のステップ

### パフォーマンス最適化

- [ ] リリースビルドを使用（`--release`）
- [ ] 複数GPUでの並列推論
- [ ] キャッシュ戦略の最適化

### 高可用性

- [ ] 複数サーバーでのロードバランシング
- [ ] ヘルスチェックとオートリカバリ
- [ ] バックアップ推論サーバーの用意

### 監視・運用

- [ ] Prometheus + Grafanaでメトリクス監視
- [ ] ログ集約（Loki, CloudWatch）
- [ ] アラート設定（レスポンスタイム、エラー率）

---

## 📚 参考リンク

- [Tailscale公式ドキュメント](https://tailscale.com/kb/)
- [Nanobot プロジェクトREADME](../README.md)
- [セキュリティベストプラクティス](./SECURITY.md)

---

**🎉 これで最もセキュアな分散推論環境が完成しました！**
