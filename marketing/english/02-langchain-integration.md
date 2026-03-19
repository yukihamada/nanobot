# Use 45+ LLM Models in LangChain with One Line Change

**TL;DR:** Point LangChain's `ChatOpenAI` at `https://api.teai.io/v1` and instantly access GPT-4o, Claude, Gemini, DeepSeek, Nemotron, and 40+ other models. No new dependencies. This article walks through RAG, agents, streaming, and a model comparison script.

---

## Why This Matters

LangChain supports multiple LLM providers, but switching between them means swapping classes (`ChatOpenAI` vs `ChatAnthropic` vs `ChatGoogleGenerativeAI`), each with different constructor args, auth patterns, and response formats.

teai.io is an OpenAI-compatible API gateway. Since LangChain's `ChatOpenAI` works with any OpenAI-compatible endpoint, you can access all 45+ models through a single class. Same interface, same streaming behavior, same structured output support.

## Setup

```bash
pip install langchain langchain-openai
```

```python
import os
os.environ["OPENAI_API_KEY"] = "your-teai-api-key"
os.environ["OPENAI_API_BASE"] = "https://api.teai.io/v1"
```

Or configure inline:

```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="your-teai-api-key",
    model="gpt-4o"
)

response = llm.invoke("What is the CAP theorem?")
print(response.content)
```

That's the one line change: `base_url="https://api.teai.io/v1"`. Everything else in your LangChain code stays the same.

## Example 1: RAG Pipeline with Model Switching

Build a simple RAG pipeline and swap the underlying LLM without touching the chain logic.

```python
from langchain_openai import ChatOpenAI, OpenAIEmbeddings
from langchain_community.vectorstores import FAISS
from langchain.text_splitter import RecursiveCharacterTextSplitter
from langchain_core.prompts import ChatPromptTemplate
from langchain_core.runnables import RunnablePassthrough
from langchain_core.output_parsers import StrOutputParser

TEAI_BASE = "https://api.teai.io/v1"
TEAI_KEY = "your-teai-api-key"

# 1. Prepare documents
docs = [
    "teai.io is an LLM API gateway based in Tokyo with <100ms proxy overhead.",
    "It supports 45+ models including GPT-4o, Claude, Gemini, and DeepSeek.",
    "Pricing uses a 5% markup. Free tier includes 1,000 credits and unlimited Nemotron 9B.",
    "The backend is built with Rust on AWS Lambda and Cloudflare Workers.",
    "BYOK (Bring Your Own Key) lets you use your existing provider API keys.",
]

splitter = RecursiveCharacterTextSplitter(chunk_size=200, chunk_overlap=20)
chunks = splitter.create_documents(docs)

# 2. Create vector store (using teai.io for embeddings too)
embeddings = OpenAIEmbeddings(
    base_url=TEAI_BASE,
    api_key=TEAI_KEY,
    model="text-embedding-3-small"
)
vectorstore = FAISS.from_documents(chunks, embeddings)
retriever = vectorstore.as_retriever(search_kwargs={"k": 3})

# 3. Build RAG chain
prompt = ChatPromptTemplate.from_template(
    "Answer based on context only.\n\nContext: {context}\n\nQuestion: {question}"
)

def format_docs(docs):
    return "\n".join(d.page_content for d in docs)

rag_chain = (
    {"context": retriever | format_docs, "question": RunnablePassthrough()}
    | prompt
    | ChatOpenAI(base_url=TEAI_BASE, api_key=TEAI_KEY, model="gpt-4o")
    | StrOutputParser()
)

# 4. Query
answer = rag_chain.invoke("What's the proxy overhead of teai.io?")
print(answer)
# Output: "teai.io has less than 100ms proxy overhead..."

# 5. Switch to Claude -- change ONLY the model parameter
rag_chain_claude = (
    {"context": retriever | format_docs, "question": RunnablePassthrough()}
    | prompt
    | ChatOpenAI(base_url=TEAI_BASE, api_key=TEAI_KEY, model="claude-sonnet-4-20250514")
    | StrOutputParser()
)
```

No class change. No import change. Just `model="claude-sonnet-4-20250514"`.

## Example 2: ReAct Agent with Tool Use

```python
from langchain_openai import ChatOpenAI
from langchain.agents import tool, AgentExecutor, create_react_agent
from langchain_core.prompts import ChatPromptTemplate

TEAI_BASE = "https://api.teai.io/v1"
TEAI_KEY = "your-teai-api-key"

@tool
def calculate(expression: str) -> str:
    """Evaluate a math expression. Input should be a valid Python expression."""
    return str(eval(expression))

@tool
def word_count(text: str) -> str:
    """Count words in text."""
    return str(len(text.split()))

llm = ChatOpenAI(
    base_url=TEAI_BASE,
    api_key=TEAI_KEY,
    model="gpt-4o",
    temperature=0
)

tools = [calculate, word_count]

# Using the newer tool-calling agent
from langchain.agents import create_tool_calling_agent

prompt = ChatPromptTemplate.from_messages([
    ("system", "You are a helpful assistant. Use tools when needed."),
    ("human", "{input}"),
    ("placeholder", "{agent_scratchpad}"),
])

agent = create_tool_calling_agent(llm, tools, prompt)
executor = AgentExecutor(agent=agent, tools=tools, verbose=True)

result = executor.invoke({"input": "What is 2**10 + 3**5? Also count words in 'hello world foo bar'."})
print(result["output"])
```

Tool calling works across GPT-4o, Claude, and Gemini through the same OpenAI-compatible function calling interface.

## Example 3: Streaming

```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="your-teai-api-key",
    model="deepseek-chat",
    streaming=True
)

for chunk in llm.stream("Write a haiku about distributed systems"):
    print(chunk.content, end="", flush=True)
```

SSE streaming works with all models that support it. teai.io proxies the stream without buffering, so first-token latency matches direct API access (plus <100ms).

## Example 4: Model Comparison Script

This is the killer use case for a gateway. Compare outputs across models for the same prompt without juggling SDKs:

```python
import time
from langchain_openai import ChatOpenAI

TEAI_BASE = "https://api.teai.io/v1"
TEAI_KEY = "your-teai-api-key"

MODELS = [
    "gpt-4o",
    "claude-sonnet-4-20250514",
    "gemini-2.0-flash",
    "deepseek-chat",
    "nvidia/llama-3.1-nemotron-70b-instruct",
]

PROMPT = "Explain the difference between async/await and threads in 3 sentences."

results = []

for model in MODELS:
    llm = ChatOpenAI(
        base_url=TEAI_BASE,
        api_key=TEAI_KEY,
        model=model,
        temperature=0
    )
    start = time.time()
    response = llm.invoke(PROMPT)
    elapsed = time.time() - start

    results.append({
        "model": model,
        "time": f"{elapsed:.2f}s",
        "tokens": response.response_metadata.get("token_usage", {}).get("completion_tokens", "N/A"),
        "output": response.content[:200]
    })

# Print comparison table
print(f"{'Model':<45} {'Time':>7} {'Tokens':>7}")
print("-" * 65)
for r in results:
    print(f"{r['model']:<45} {r['time']:>7} {str(r['tokens']):>7}")
    print(f"  {r['output']}...")
    print()
```

Run this once and you'll know which model fits your use case best -- quality, speed, and cost -- without writing provider-specific code.

## Example 5: Structured Output

```python
from langchain_openai import ChatOpenAI
from pydantic import BaseModel, Field

class MovieReview(BaseModel):
    title: str = Field(description="Movie title")
    rating: float = Field(description="Rating out of 10")
    summary: str = Field(description="One sentence summary")

llm = ChatOpenAI(
    base_url="https://api.teai.io/v1",
    api_key="your-teai-api-key",
    model="gpt-4o"
)

structured_llm = llm.with_structured_output(MovieReview)
review = structured_llm.invoke("Review the movie 'Arrival' (2016)")
print(f"{review.title}: {review.rating}/10 - {review.summary}")
```

Structured output via JSON mode and function calling works through the gateway just like direct API access.

## Cost Optimization Tip

Use different models for different chain steps:

```python
# Cheap model for summarization/extraction
fast_llm = ChatOpenAI(base_url=TEAI_BASE, api_key=TEAI_KEY, model="deepseek-chat")

# Premium model for final reasoning
smart_llm = ChatOpenAI(base_url=TEAI_BASE, api_key=TEAI_KEY, model="gpt-4o")

# Or use the free Nemotron 9B for development/testing
dev_llm = ChatOpenAI(base_url=TEAI_BASE, api_key=TEAI_KEY, model="nemotron-9b")
```

Nemotron 9B is free and unlimited on teai.io -- use it for development and testing, then swap to a premium model for production.

## Get Started

1. Sign up at [teai.io](https://teai.io) -- free, no credit card
2. `pip install langchain langchain-openai`
3. Set `base_url="https://api.teai.io/v1"` in `ChatOpenAI`
4. Access 45+ models through one interface

Full model list and pricing at [teai.io/models](https://teai.io/models).

---

*teai.io is an LLM API gateway built with Rust on AWS Lambda (Tokyo). OpenAI-compatible API, 45+ models, <100ms proxy overhead.*
