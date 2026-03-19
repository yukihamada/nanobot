---
title: "OpenAI APIより安くて速い？日本発LLM Gateway「teai.io」を使ってみた"
tags: ["OpenAI", "LLM", "AI", "API", "Python"]
---

# OpenAI APIより安くて速い？日本発LLM Gateway「teai.io」を使ってみた

## TL;DR

- OpenAI互換のLLM API Gateway **teai.io** を紹介
- `base_url` を1行変えるだけで **GPT-4o、Claude、Gemini、Nemotron** 等45+モデルが使える
- 東京サーバーで **レイテンシ100ms以下**（OpenRouterより速い）
- マークアップ **5%**（OpenRouter 5.5%より安い）
- **無料枠あり**、Nemotron 9Bは無制限

## なぜ今、LLM API Gatewayが必要なのか

LLMアプリを開発していると、こんな課題にぶつかりませんか？

1. **モデルの切り替えが面倒** — OpenAIからClaudeに変えたいけど、SDKもAPIも違う
2. **コスト管理が難しい** — 複数プロバイダの請求がバラバラ
3. **障害時の対応** — OpenAIが落ちたらアプリ全停止
4. **レイテンシ** — 海外サーバー経由で遅い

LLM API Gatewayはこれらを一括解決します。海外では **OpenRouter**（$5M ARR、250万ユーザー）が有名ですが、日本向けに最適化されたサービスはこれまでありませんでした。

## teai.io とは

**teai.io** は、日本のAI開発者向けに設計されたLLM API Gatewayです。

| 特徴 | teai.io | OpenRouter |
|------|---------|------------|
| サーバー位置 | **東京** | 米国 |
| レイテンシオーバーヘッド | **<100ms** | 200-400ms |
| マークアップ | **5%** | 5.5% |
| 日本語ドキュメント | **あり** | なし |
| 円建て請求 | **あり** | なし |
| インボイス制度対応 | **あり** | なし |
| 無料モデル | **Nemotron 9B** | 一部あり |

## セットアップ（3分）

### Step 1: APIキーを取得

[teai.io/register](https://teai.io/register) でアカウント作成。無料で1,000クレジットがもらえます。

### Step 2: コードを1行変更

既存のOpenAI SDKコードがそのまま使えます。変えるのは `base_url` だけ。

```python
from openai import OpenAI

client = OpenAI(
    base_url="https://api.teai.io/v1",  # ← この1行だけ
    api_key="te_your_api_key"
)

response = client.chat.completions.create(
    model="gpt-4o",  # or "claude-sonnet-4-6", "gemini-2.5-pro" etc.
    messages=[
        {"role": "user", "content": "東京でおすすめのラーメン屋を3つ教えて"}
    ]
)

print(response.choices[0].message.content)
```

### Step 3: 他のモデルも試す

モデル名を変えるだけで、異なるプロバイダのモデルを切り替えられます。

```python
# Claude Sonnet 4.6 を使う
response = client.chat.completions.create(
    model="claude-sonnet-4-6",
    messages=[{"role": "user", "content": "Pythonで素数判定関数を書いて"}]
)

# Gemini 2.5 Pro を使う
response = client.chat.completions.create(
    model="gemini-2.5-pro",
    messages=[{"role": "user", "content": "React vs Vue、2026年のおすすめは？"}]
)

# 無料のNemotron 9B（日本語特化）を使う
response = client.chat.completions.create(
    model="nemotron-9b",
    messages=[{"role": "user", "content": "確定申告の流れを教えて"}]
)
```

## モデルと料金

主要モデルの料金（1Mトークンあたり、円建て）：

| モデル | 入力 | 出力 | 特徴 |
|--------|------|------|------|
| **Nemotron 9B** | **¥0** | **¥0** | 日本語特化、無制限 |
| Gemini 2.5 Flash | ¥15 | ¥60 | コスパ最強 |
| GPT-4o-mini | ¥22 | ¥90 | 軽量で高速 |
| Claude Haiku 4.5 | ¥150 | ¥750 | 高品質で安い |
| GPT-4o | ¥375 | ¥1,500 | バランス型 |
| Claude Sonnet 4.6 | ¥450 | ¥2,250 | コーディング最強 |
| Gemini 2.5 Pro | ¥187 | ¥1,500 | 長文処理が得意 |

> 料金は各プロバイダの原価 + 5%マージンのみ。隠れたコストはありません。

## ストリーミング対応

リアルタイムストリーミングも標準対応。チャットボットやAIアシスタントの開発に最適です。

```python
stream = client.chat.completions.create(
    model="claude-sonnet-4-6",
    messages=[{"role": "user", "content": "AIの歴史を500字で"}],
    stream=True
)

for chunk in stream:
    if chunk.choices[0].delta.content:
        print(chunk.choices[0].delta.content, end="")
```

## Tool Calling（Function Calling）

OpenAI互換のTool Callingもそのまま使えます。

```python
tools = [
    {
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "指定された都市の天気を取得",
            "parameters": {
                "type": "object",
                "properties": {
                    "city": {"type": "string", "description": "都市名"}
                },
                "required": ["city"]
            }
        }
    }
]

response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "東京の天気は？"}],
    tools=tools,
    tool_choice="auto"
)
```

## 自動フォールバック

teai.ioは複数のバックエンドを持ち、あるプロバイダが障害を起こしても自動的に代替モデルにフォールバックします。

```
リクエスト → GPT-4o (OpenAI)
               ↓ 障害時
             Claude Sonnet (Anthropic)
               ↓ 障害時
             Gemini Pro (Google)
```

アプリ側のコード変更は不要。99.9%のアップタイムを実現します。

## BYOK（Bring Your Own Key）

自分のAPIキーを使いたい場合も対応。マージン0%で利用できます。

```python
client = OpenAI(
    base_url="https://api.teai.io/v1",
    api_key="te_your_api_key",
    default_headers={
        "X-OpenAI-Key": "sk-your-openai-key"  # 自分のOpenAIキー
    }
)
```

## まとめ

| メリット | 詳細 |
|---------|------|
| **速い** | 東京サーバー、100ms以下のオーバーヘッド |
| **安い** | 5%マージンのみ、Nemotron 9Bは無料 |
| **簡単** | OpenAI SDK互換、1行変更で移行完了 |
| **安全** | プロンプト保存なし、TLS暗号化 |
| **柔軟** | 45+モデル、自動フォールバック |

**無料で1,000クレジット**がもらえるので、まずは試してみてください。

- サイト: [teai.io](https://teai.io)
- API Docs: [teai.io/docs](https://teai.io/docs)
- 登録: [teai.io/register](https://teai.io/register)

---

*この記事が参考になったら「いいね」をお願いします！質問があればコメントでどうぞ。*
