---
paths:
  - "web/**/*.html"
  - "web/**/*.css"
  - "web/**/*.js"
---
# Web Frontend Rules

## SPA構成
- 全てインラインSPA（CSS + HTML + JS が1ファイル）
- `include_str!()` でRustバイナリに埋込 → **HTML変更後は `cargo build` 必須**

## Voice-first UI
- 空状態: 大きなマイクボタンが中央に配置
- STT: Web Speech API (ja-JP)、Chrome/Edgeのみ
- TTS: `/api/v1/speech/synthesize` → MP3再生 + ブラウザキャッシュ
- Auto-TTS: 音声入力 → 応答を自動再生
- `appAddMsg()` でbotメッセージにTTSボタン自動付与

## SSEストリーミング
- `/api/v1/chat/stream` でリアルタイム応答
- `processEvent()` がJSON配列を展開して各イベント処理
- イベント: start, tool_start, tool_result, thinking, content, error, done
- `.agent-progress` CSSクラスでツール実行ステップ表示

## ブランディング
- **chatweb.ai** (index.html): 音声中心、一般ユーザー向け、インディゴ(#6366f1)
- **teai.io** (teai-*.html): 開発者向け、グリーン(#10b981)
- 共有バックエンド、別UI・ブランド

## スマホ最適化
- safe-area-inset対応
- タッチターゲット最小38px
- 地域別チャネル順序: 日本=LINE優先 / 海外=WhatsApp優先
