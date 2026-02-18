# OpenAI Realtime API 統合評価

**評価日**: 2026-02-17
**評価者**: Claude Code
**対象**: chatweb.ai / nanobot プロジェクト

## 概要

OpenAI Realtime APIは、WebSocket経由でリアルタイム双方向音声会話を提供するAPIです。従来のREST API（STT → LLM → TTS）と比較して、統合された音声体験を実現します。

## 主な機能

### 1. **リアルタイム双方向音声**
- WebSocketベースの常時接続
- ストリーミング音声入力（マイク → API）
- ストリーミング音声出力（API → スピーカー）
- レイテンシ: 200-500ms（従来の1-3秒から大幅改善）

### 2. **統合されたSTT + LLM + TTS**
- 単一APIコールで全プロセス処理
- 中間テキスト不要（音声 → 音声）
- モデル: `gpt-4o-realtime-preview`

### 3. **ネイティブ割り込み処理**
- ユーザーが話し始めると自動で音声出力停止
- `conversation.item.truncate` イベントで制御
- VAD（Voice Activity Detection）内蔵

### 4. **ワードレベルタイムスタンプ**
- 音声とテキストの同期
- リップシンク、字幕表示に対応
- `response.audio.delta` + `response.text.delta` イベント

### 5. **マルチモーダル対応**
- 音声 + テキスト同時入力
- 画像入力（vision機能）
- Function calling（ツール実行）

## 現在のnanobot実装との比較

| 機能 | 現在の実装 | Realtime API |
|------|------------|--------------|
| **アーキテクチャ** | REST API (3段階) | WebSocket (統合) |
| **レイテンシ** | 1-3秒 | 200-500ms |
| **STT** | Web Speech API / Whisper | 内蔵 |
| **LLM** | Multi-provider (Anthropic, OpenAI, Google, etc.) | GPT-4o のみ |
| **TTS** | Replicate Qwen3 / OpenAI / ElevenLabs / Polly | 内蔵 (OpenAI voices) |
| **割り込み処理** | フロントエンド実装 (stopTTS on start) | ネイティブサポート |
| **ストリーミング** | SSE (テキストのみ) | WebSocket (音声+テキスト) |
| **音声品質** | 複数エンジン選択可 | OpenAI voicesのみ |
| **コスト** | $0.006/min (入力) + $0.024/min (出力) | $0.06/min (音声) + $0.001/1K tokens (テキスト) |
| **マルチチャネル** | Web, LINE, Telegram, Facebook | Webのみ（音声会話） |

## メリット

### ✅ **1. 大幅なレイテンシ削減**
- 現在: STT (500ms) + LLM (1-2s) + TTS (500ms) = 2-3秒
- Realtime: 統合処理で200-500ms（最大6倍高速化）

### ✅ **2. 自然な会話体験**
- ネイティブ割り込み処理（フィラー音声不要）
- 低レイテンシによる応答性向上
- ワードレベル同期（リップシンク、字幕）

### ✅ **3. 実装の簡素化**
- STT/TTS統合による複雑性削減
- VAD、割り込み処理の標準化
- エラーハンドリング簡素化

### ✅ **4. Function Calling統合**
- 音声会話中にツール実行可能
- 現在のagentic modeと統合可能

## デメリット

### ❌ **1. LLMプロバイダーのロックイン**
- GPT-4oのみ（現在はマルチプロバイダー）
- Anthropic Claude、Google Geminiが使えない
- kimi k2.5、DeepSeek等のカスタムモデル不可

### ❌ **2. コスト増加**
- 音声会話: $0.06/min vs 現在 $0.03/min（約2倍）
- テキストチャット: 変化なし

### ❌ **3. マルチチャネル非対応**
- LINE、Telegram、Facebookは従来のREST API継続必須
- 2つの実装パスを維持する必要あり

### ❌ **4. 音声エンジン選択不可**
- OpenAI voicesのみ
- 現在: Replicate Qwen3 / ElevenLabs / Style-Bert-VITS2 / CosyVoice / Polly等

### ❌ **5. 実装複雑性**
- WebSocket管理（再接続、エラーハンドリング）
- Lambda非対応（常時接続不可） → 別インフラ必要
- バイナリ音声データのストリーミング処理

## アーキテクチャ提案

### **ハイブリッドアプローチ（推奨）**

```
                 ┌─────────────────┐
                 │   Web Browser   │
                 └────────┬────────┘
                          │
              ┌───────────┴───────────┐
              │                       │
         【音声会話】             【テキストチャット】
              │                       │
    ┌─────────▼─────────┐   ┌────────▼────────┐
    │ Realtime API (WS) │   │ REST API (HTTP) │
    │  - GPT-4o only    │   │  - Multi-model  │
    │  - Low latency    │   │  - Agentic mode │
    │  - Voice native   │   │  - All channels │
    └───────────────────┘   └─────────────────┘
              │                       │
         Fly.io/EC2              AWS Lambda
        (WebSocket)             (現在のまま)
```

### **実装フェーズ**

#### **Phase 1: プロトタイプ（1週間）**
- Fly.ioにWebSocketサーバー構築
- Realtime API基本統合（音声入出力）
- Web UIにWebSocket接続追加（feature flag）

#### **Phase 2: 本番投入（2週間）**
- エラーハンドリング、再接続ロジック
- Function calling統合（ツール実行）
- ログ、メトリクス（会話品質、レイテンシ）

#### **Phase 3: 最適化（継続）**
- 音声品質チューニング
- コスト最適化（idle timeout、session pooling）
- A/Bテスト（Realtime vs 従来）

## コスト分析

### **想定使用量**
- アクティブユーザー: 1,000人/月
- 平均会話時間: 5分/セッション
- 音声会話比率: 30%（残り70%はテキスト）

### **現在のコスト（REST API）**
```
音声会話:
  STT (Whisper): $0.006/min
  LLM (GPT-4o-mini): $0.15/1M tokens ≈ $0.003/min
  TTS (OpenAI): $15/1M chars ≈ $0.02/min
  合計: $0.029/min

月間: 1,000 users × 5 min × 30% × $0.029 = $43.5
```

### **Realtime API コスト**
```
音声会話:
  Realtime API: $0.06/min (音声) + $0.001/1K tokens (テキスト)
  合計: 約 $0.065/min

月間: 1,000 users × 5 min × 30% × $0.065 = $97.5
```

**増加額**: +$54/月（約2.2倍）

### **ROI評価**
- レイテンシ削減による体験向上 → 離脱率低下、継続率向上
- 音声会話の増加 → エンゲージメント向上 → 課金転換率向上
- コスト増加: $54/月 → 新規課金ユーザー1人で回収可能（Starter: $9/月 × 6ヶ月）

## リスク評価

| リスク | 影響度 | 発生確率 | 対策 |
|--------|--------|----------|------|
| GPT-4oダウンタイム | 高 | 低 | REST APIフォールバック |
| レイテンシ期待外れ | 中 | 低 | A/Bテスト、段階的ロールアウト |
| コスト超過 | 中 | 中 | Idle timeout、使用量監視、アラート |
| WebSocket実装バグ | 高 | 中 | 徹底したテスト、段階的リリース |
| Lambdaからの移行 | 高 | 低 | Fly.io並行運用（既存Lambda維持） |

## 推奨事項

### **✅ 統合を推奨**

**理由:**
1. **chatweb.aiのコアバリュー**は「音声中心のAI体験」 → Realtime APIと完全一致
2. レイテンシ削減（2-3秒 → 500ms）は**決定的な競争優位性**
3. コスト増加（+$54/月）は**許容範囲内**、ROI見込み高い
4. ハイブリッドアプローチでリスク最小化（既存REST API維持）

### **実装計画**

#### **Week 1: プロトタイプ**
- [ ] Fly.ioにNode.js WebSocketサーバー構築
- [ ] Realtime API基本統合（echo test）
- [ ] Web UIにWebSocket接続追加（`?realtime=1` feature flag）

#### **Week 2: 統合**
- [ ] Function calling統合（web_search, calculator等）
- [ ] セッション管理（会話履歴、メモリ注入）
- [ ] エラーハンドリング、再接続ロジック

#### **Week 3: テスト**
- [ ] 負荷テスト（同時接続100+）
- [ ] A/Bテスト設定（50% Realtime / 50% REST）
- [ ] レイテンシ、音声品質測定

#### **Week 4: 本番投入**
- [ ] Pro/Enterpriseプランで先行リリース
- [ ] ログ、メトリクス収集（CloudWatch, Datadog）
- [ ] ドキュメント更新（CLAUDE.md, README）

### **成功指標**

- **レイテンシ**: TTFB < 500ms（現在 2-3秒）
- **音声会話比率**: 30% → 50%（体験向上による増加）
- **継続率**: +10%（低レイテンシによる離脱減）
- **NPS**: +5pt（ユーザー満足度向上）

## 結論

OpenAI Realtime APIの統合は、**chatweb.aiのビジョン（音声中心のAI体験）と完全に一致**し、決定的な競争優位性をもたらします。コスト増加は許容範囲内であり、ハイブリッドアプローチにより既存機能を維持しながらリスクを最小化できます。

**推奨**: Phase 1プロトタイプを即座に開始し、4週間で本番投入を目指す。

---

## 参考資料

- [OpenAI Realtime API Documentation](https://platform.openai.com/docs/guides/realtime)
- [Realtime API Pricing](https://openai.com/api/pricing/)
- [WebRTC vs WebSocket for Voice](https://blog.livekit.io/webrtc-vs-websocket/)
- [Fly.io WebSocket Support](https://fly.io/docs/app-guides/websockets/)

## 次のステップ

1. ✅ この評価ドキュメントをレビュー
2. [ ] ユーザーに承認を得る
3. [ ] Fly.ioアカウント準備（既存: teai-io app利用可能）
4. [ ] OpenAI Realtime API キー取得
5. [ ] Phase 1プロトタイプ開始
