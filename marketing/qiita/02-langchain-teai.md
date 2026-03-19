---
title: "LangChain × teai.io：1行変えるだけで45+モデルが使い放題になる方法"
tags: ["LangChain", "Python", "LLM", "AI", "OpenAI"]
---

# LangChain × teai.io：1行変えるだけで45+モデルが使い放題になる方法

## はじめに

LangChainでLLMアプリを開発するとき、こんな経験ありませんか？

- OpenAIのAPI料金が高くて、開発中のテストに気を使う
- 「Claudeの方が精度良いかも」と思っても、移行が面倒
- モデルA/Bテストしたいけど、プロバイダごとにコード変更が必要

**teai.io** を使えば、LangChainの `base_url` を1行変えるだけで、GPT-4o、Claude、Gemini、DeepSeek、Nemotron等の45+モデルを統一APIで使えるようになります。

## セットアップ

```bash
pip install langchain-openai
```

```python
from langchain_openai import ChatOpenAI

# teai.io経由でどのモデルも使える
llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",  # ← この1行追加
    api_key="te_your_api_key",           # teai.ioで無料取得
    model="claude-sonnet-4-6"            # 好きなモデルを指定
)

response = llm.invoke("LangChainとは何ですか？")
print(response.content)
```

これだけです。LangChainの他の機能（Chain、Agent、RAG等）はすべてそのまま動きます。

## 実践例1: モデル比較ツール

同じプロンプトを複数モデルに投げて、品質・速度・コストを比較するスクリプト：

```python
import time
from langchain_openai import ChatOpenAI

MODELS = [
    ("nemotron-9b", "無料・日本語特化"),
    ("gemini-2.5-flash", "コスパ最強"),
    ("gpt-4o-mini", "高速バランス"),
    ("claude-sonnet-4-6", "高品質"),
]

prompt = "Pythonのデコレータを初心者にわかりやすく説明してください。"

for model_name, description in MODELS:
    llm = ChatOpenAI(
        base_url="https://api.teai.io/v1",
        api_key="te_your_api_key",
        model=model_name
    )

    start = time.time()
    response = llm.invoke(prompt)
    elapsed = time.time() - start

    print(f"\n{'='*60}")
    print(f"Model: {model_name} ({description})")
    print(f"Time: {elapsed:.2f}s")
    print(f"Length: {len(response.content)} chars")
    print(f"{'='*60}")
    print(response.content[:300] + "...")
```

> teai.io なら、モデルの比較検討がコード変更なしで可能。最適なモデルを見つけたら、本番でそのまま使えます。

## 実践例2: RAG（検索拡張生成）

```python
from langchain_openai import ChatOpenAI, OpenAIEmbeddings
from langchain_community.vectorstores import FAISS
from langchain.text_splitter import RecursiveCharacterTextSplitter
from langchain.chains import RetrievalQA

# Embeddingsもteai.io経由
embeddings = OpenAIEmbeddings(
    base_url="https://api.teai.io/v1",
    api_key="te_your_api_key"
)

# テキストをチャンクに分割
text_splitter = RecursiveCharacterTextSplitter(
    chunk_size=500,
    chunk_overlap=50
)
docs = text_splitter.create_documents([
    "teai.ioは日本発のLLM API Gatewayです。",
    "45以上のモデルをOpenAI互換APIで提供します。",
    "東京リージョンで低レイテンシを実現しています。",
])

# ベクトルストア作成
vectorstore = FAISS.from_documents(docs, embeddings)

# RAGチェーン構築
llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="te_your_api_key",
    model="gemini-2.5-flash"  # コスパ重視ならGemini Flash
)

qa_chain = RetrievalQA.from_chain_type(
    llm=llm,
    retriever=vectorstore.as_retriever()
)

result = qa_chain.invoke("teai.ioの特徴は？")
print(result["result"])
```

## 実践例3: エージェント with Tool Calling

```python
from langchain_openai import ChatOpenAI
from langchain.agents import AgentExecutor, create_openai_tools_agent
from langchain_core.prompts import ChatPromptTemplate
from langchain_core.tools import tool

@tool
def calculate(expression: str) -> str:
    """数式を計算する"""
    return str(eval(expression))

@tool
def search_web(query: str) -> str:
    """ウェブ検索をシミュレートする"""
    return f"「{query}」の検索結果: ..."

# teai.io経由でGPT-4oのTool Calling
llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="te_your_api_key",
    model="gpt-4o"
)

prompt = ChatPromptTemplate.from_messages([
    ("system", "あなたは便利なアシスタントです。ツールを使って正確に回答してください。"),
    ("human", "{input}"),
    ("placeholder", "{agent_scratchpad}"),
])

agent = create_openai_tools_agent(llm, [calculate, search_web], prompt)
executor = AgentExecutor(agent=agent, tools=[calculate, search_web])

result = executor.invoke({"input": "2の10乗は？"})
print(result["output"])
```

## 実践例4: ストリーミングチャットボット

```python
from langchain_openai import ChatOpenAI
from langchain_core.messages import HumanMessage, SystemMessage

llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="te_your_api_key",
    model="claude-sonnet-4-6",
    streaming=True
)

messages = [
    SystemMessage(content="あなたは親切なAIアシスタントです。"),
    HumanMessage(content="FastAPIでWebSocketチャットを作る方法を教えて")
]

# ストリーミングで1トークンずつ出力
for chunk in llm.stream(messages):
    print(chunk.content, end="", flush=True)
```

## コスト比較

LangChainで1日100リクエスト（各1,000トークン）を30日間使った場合の月額比較：

| 構成 | 月額概算 |
|------|---------|
| OpenAI API直接 (GPT-4o) | ~$15 |
| OpenRouter経由 (GPT-4o) | ~$15.80 (+5.5%) |
| **teai.io経由 (GPT-4o)** | **~$15.75 (+5%)**  |
| **teai.io (Gemini Flash)** | **~$1.20** |
| **teai.io (Nemotron 9B)** | **¥0（無料）** |

> ポイント：teai.ioならモデルを自由に切り替えられるので、開発中はNemotron（無料）、本番はGPT-4oやClaudeという使い分けが簡単。

## まとめ

LangChain × teai.io の組み合わせで得られるメリット：

1. **1行変更で45+モデル** — `base_url` を変えるだけ
2. **コスト削減** — 開発時は無料のNemotron、本番は最適モデル
3. **低レイテンシ** — 東京サーバーで日本からのアクセス高速
4. **障害耐性** — 自動フォールバックでダウンタイムなし
5. **円建て請求** — 為替リスクなし、インボイス対応

```python
# たったこれだけ
llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="te_your_api_key",
    model="your-choice"
)
```

無料で始められるので、ぜひ試してみてください → [teai.io](https://teai.io)

---

*LangChain × teai.io で何か面白いもの作ったら、ぜひコメントで教えてください！*
