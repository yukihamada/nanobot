# Todo — 課金転換率改善プロジェクト（最終更新: 2026-03-01）

## 1. オンボーディング改善
- [x] 初回訪問時のウェルカムメッセージ改善（サジェストピル追加）
- [x] 機能デモ（音声入力・画像生成等）の自動提案（ピルクリックで送信）
- [x] プログレスバー（残りクレジット表示）（ヘッダーに4pxバー追加）

## 2. 課金導線の強化
- [x] クレジット残少時のソフトナッジ（残り20%で赤、40%でオレンジ表示）
- [ ] クレジット切れ時のアップグレードモーダル改善
- [x] 価値体験後の自然な誘導（画像/音楽/動画生成完了後のソフトアップセル）

## 3. リテンション施策
- [ ] LINE/Telegram でのデイリーサマリー通知（要スケジュール基盤）
- [ ] 「昨日の会話の続き」導線

## 4. SEO / ランディングページ
- [x] meta tags, OGP, structured data（完備: OGP, Twitter Cards, JSON-LD, hreflang 7言語）
- [x] ヒーローセクション刷新（統計数値更新: 10+モデル, 14+チャネル, 30+ツール, 500+ユーザー）
- [ ] 機能紹介セクション追加

## 5. ボイス体験の磨き込み
- [x] TTS応答速度の最適化（Replicate+QWENを並列レース化）
- [x] 音声入力→応答のレイテンシ改善（web_search並列化、agentic deadline追加）

## 6. モデル選択の動的化（追加完了）
- [x] PRICING_TABLEをSSoTとしてAPI `/api/v1/models` から全モデル返却
- [x] フロントエンドのドロップダウンが動的フェッチ（フォールバック付き）
- [x] OpenAI互換 `/v1/models` もPRICING_TABLEから動的生成

## 7. Nemotron ツール名修正（2026-03-01 追加、v139）
- [x] pricing.rs ケースセンシティブバグ修正（Nemotron 4.4x過課金 → 修正済み、v138）
- [x] web_fetch → read_webpage, qr_code → create_qr リネーム（Nemotronが認識できない名前を修正）
- [x] AGENT_COMMON プロンプト: ツール一覧と優先使用の指示を追加
- [x] tests/test_capabilities.sh 20項目テストスクリプト作成
- [x] date_time → datetime ツール名不一致バグ修正（v140予定）
- [x] tool descriptions のレガシー名参照修正（integrations.rs, v140予定）
