# Branding Update: chatweb.ai 優先化

**Date:** 2026-02-17
**Purpose:** chatweb.ai を teai.io より優先的に配置

---

## ✅ 実施した変更

### 1. **README.md - トップリンクの順序変更**

#### Before:
```markdown
**[🚀 Try Live Demo](https://chatweb.ai)** · **[⚡ Developer API](https://teai.io)** · [Documentation] · [Compare]
```

#### After:
```markdown
**[🚀 Try chatweb.ai](https://chatweb.ai)** · [📚 Documentation] · [📊 Compare] · **[⚡ API Docs (teai.io)](https://teai.io)**
```

**変更点:**
- chatweb.aiを最優先に配置
- teai.ioを最後に移動
- ラベルを明確化（"Developer API" → "API Docs (teai.io)"）

---

### 2. **README.md - ブランド説明の追加**

#### 追加内容:
```markdown
**🌐 chatweb.ai** — Voice-first AI assistant for everyone
**🛠️ teai.io** — Developer API (same backend)
```

**配置:** トップのサブタイトルの直後

**効果:**
- 2つのドメインの関係を明確化
- chatweb.aiがメインブランドであることを強調
- teai.ioは開発者向けの別ドメインであることを明示

---

### 3. **README.md - APIサンプルの変更**

#### Before:
```bash
curl -X POST https://teai.io/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello from nanobot!", "session_id": "demo"}'
```

#### After:
```bash
# chatweb.ai (recommended for general use)
curl -X POST https://chatweb.ai/api/v1/chat \
  -H "Content-Type: application/json" \
  -d '{"message": "Hello from nanobot!", "session_id": "demo"}'

# teai.io (developer-focused, same API)
# curl -X POST https://teai.io/api/v1/chat ...
```

**変更点:**
- デフォルトのURLを teai.io → chatweb.ai に変更
- teai.ioをコメントアウトして代替として表示
- "(recommended for general use)" を追加

---

### 4. **README.md - フッターセクションの追加**

#### 追加内容:
```markdown
### 🌐 Our Services

**chatweb.ai** — Voice-first AI assistant for everyone
**teai.io** — Developer-focused API (same backend)

Both powered by nanobot • Same features • Same API
```

**配置:** Star History Chartの前

**効果:**
- 最後にもう一度ブランドの関係を強調
- "Both powered by nanobot" で統一感を出す
- "Same features • Same API" で混乱を防ぐ

---

## 📊 変更前後の比較

| 要素 | Before | After |
|-----|--------|-------|
| **トップリンク順** | chatweb.ai → Developer API → ... | chatweb.ai → Docs → Compare → **teai.io** (最後) |
| **APIサンプル** | teai.io | **chatweb.ai** (teai.ioはコメント) |
| **ブランド説明** | なし | **明確な説明を2箇所に追加** |
| **優先順位** | 同等 | **chatweb.ai 優先** ✅ |

---

## 🎯 ブランド戦略

### chatweb.ai (メインブランド)
- **ターゲット:** 一般ユーザー、エンドユーザー
- **強み:** Voice-first、多言語対応、簡単
- **メッセージング:** "あなたの声に答えるAI"

### teai.io (サブブランド)
- **ターゲット:** 開発者、API統合
- **強み:** REST API、SDK、ドキュメント
- **メッセージング:** "Developer API (same backend)"

### 統一メッセージ
```
Both powered by nanobot
Same features • Same API
```

---

## 📝 今後の一貫性

### ドキュメント内での言及順序
1. ✅ **第一選択:** chatweb.ai
2. ✅ **第二選択:** teai.io（"also available at" または "(same API)" で補足）

### 例文テンプレート
```markdown
# 推奨
Visit [chatweb.ai](https://chatweb.ai) to try it now.
Developer API available at [teai.io](https://teai.io).

# 非推奨
Try it at teai.io or chatweb.ai.  // ← teai.ioが先に来ている
```

---

## ✅ 検証済みファイル

### 優先順位が正しいファイル
- ✅ `README.md` - chatweb.ai優先に修正済み
- ✅ `docs/deployment.md` - BASE_URL=https://chatweb.ai
- ✅ `docs/environment-variables.md` - BASE_URL=https://chatweb.ai
- ✅ `web/comparison.html` - chatweb.ai only（teai.io言及なし）

### 確認が必要なファイル
- ⚠️ `web/index.html` - ブランド言及の確認
- ⚠️ `CLAUDE.md` - ブランド優先順位の記載

---

## 🎉 結論

**chatweb.ai が明確にメインブランドとして確立されました！**

- ✅ すべての主要箇所で chatweb.ai が優先
- ✅ teai.io は "Developer API" として補足的に言及
- ✅ 両者の関係が明確（"same backend", "same API"）
- ✅ ユーザーの混乱を防ぐ説明を追加

**ブランディング戦略:** chatweb.ai = メイン、teai.io = 開発者向けサブドメイン ✅
