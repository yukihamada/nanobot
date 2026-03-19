# teai.io Twitter/X 投稿テンプレート

## ローンチ告知

### 投稿1: メイン告知
```
日本の開発者向けLLM API Gateway「teai.io」を公開しました

- OpenAI互換API（1行変更で移行）
- 45+モデル（GPT-4o, Claude, Gemini, DeepSeek...）
- 東京サーバー（100ms以下のオーバーヘッド）
- Nemotron 9B 無料・無制限
- 円建て請求・インボイス対応

無料で1,000クレジット
https://teai.io

#AI開発 #LLM #OpenAI #Claude #生成AI
```

### 投稿2: コード例
```
OpenAI APIのコードを1行変えるだけで、45+モデルが使える

from openai import OpenAI
client = OpenAI(
    base_url="https://api.teai.io/v1",  # これだけ
    api_key="te_your_key"
)

GPT-4o, Claude, Gemini, DeepSeek...
全部同じAPIで。東京サーバーで高速。

https://teai.io
#Python #AI #LLM
```

### 投稿3: OpenRouter比較
```
日本からOpenRouter使ってる開発者へ

teai.io なら：
- 東京サーバーで500ms速い
- マークアップ5%（vs 5.5%）
- 日本語ドキュメント完備
- 円建て請求・インボイス対応
- Nemotron 9B 無料

OpenAI SDK互換で移行1行。

https://teai.io
#AI開発 #LLM #APIGateway
```

### 投稿4: 無料訴求
```
LLM APIを無料で使い倒す方法

teai.io に登録するだけで：
1. 1,000クレジット無料
2. Nemotron 9B 無制限（日本語強い）
3. クレジットカード不要

チャットボット、翻訳、コードレビュー...
無料で全部できる。

https://teai.io/register
#AI #無料 #LLM #生成AI
```

### 投稿5: 技術記事シェア
```
teai.ioのアーキテクチャを全公開しました

Rust + AWS Lambda + Cloudflare Workers で
LLM API Gatewayを構築した話。

- コールドスタート80ms
- musl vs gnu の罠
- SSEストリーミング中継
- 月額$5のインフラコスト

https://zenn.dev/teai/articles/teai-architecture
#Rust #AWS #Lambda #アーキテクチャ
```

### 投稿6: B2B訴求
```
法人でLLM APIを使うなら

teai.io は日本企業のために設計：
- 適格請求書（インボイス）発行
- 円建て決済
- 日本語契約書・NDA
- SLA 99.9%保証
- 東京リージョン

経理処理の手間ゼロ。

https://teai.io
#AI #B2B #SaaS #インボイス制度
```

## 定期投稿（週1-2回）

### パターンA: Tips
```
[Tips] LangChainでモデルを切り替えるとき、
プロバイダごとにSDKを変える必要はありません

base_url="https://api.teai.io/v1" にするだけで
GPT-4o → Claude → Gemini を自由に切替可能

https://teai.io/docs
#LangChain #AI #開発Tips
```

### パターンB: モデル紹介
```
[新モデル] Gemini 2.5 Flash が teai.io で利用可能に

入力: ¥15/1Mトークン
出力: ¥60/1Mトークン

GPT-4o-miniの半額以下で、かなり使える。
コスパ重視のプロジェクトにおすすめ。

https://teai.io
```

### パターンC: ユースケース
```
[ユースケース] git diffをAIにレビューさせるスクリプト、10行で書ける

teai.io + Nemotron 9B（無料）で
コードレビューBotを作る方法

詳しくは記事で↓
https://qiita.com/teai/items/free-llm-nemotron
```

## ハッシュタグ戦略
- 必須: #AI開発 #LLM
- ローテーション: #生成AI #OpenAI #Claude #Python #Rust #APIGateway #無料
- トレンド便乗: #ChatGPT #Claude4 #Gemini（新モデルリリース時）
