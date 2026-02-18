# CosyVoice 2 TTS for RunPod

高品質な音声合成と音声クローニング。Alibaba提供のCosyVoice 2モデル。

## 特徴
- **Zero-shot音声クローニング**: 3-10秒のサンプルから声を再現
- **多言語対応**: 日本語、英語、中国語
- **スタイル制御**: Instruct モードで話し方を指定可能
- **高品質**: 22kHz サンプリングレート

## デプロイ

### 1. Dockerイメージをビルド
```bash
cd infra/runpod/cosyvoice
docker build -t YOUR_DOCKERHUB/cosyvoice-tts:latest .
docker push YOUR_DOCKERHUB/cosyvoice-tts:latest
```

### 2. RunPodでデプロイ
1. RunPod Console → Serverless → Endpoints
2. New Template:
   - **Container Image**: `YOUR_DOCKERHUB/cosyvoice-tts:latest`
   - **Container Disk**: 30GB (モデル2GB + ワーク領域)
   - **GPU**: A10 (24GB) または A100
   - **Environment Variables**: `MODEL_DIR=/root/.cache/modelscope/iic/CosyVoice2-0___5B`
3. New Endpoint:
   - **Template**: 上記で作成したテンプレート
   - **Active Workers**: 0-1 (オートスケール)
   - **Max Workers**: 3
   - **Idle Timeout**: 300秒

### 3. Lambda環境変数に追加
```bash
# Endpoint URLを取得
RUNPOD_ENDPOINT_ID="your_endpoint_id"

aws lambda update-function-configuration \
  --function-name nanobot \
  --region ap-northeast-1 \
  --environment "Variables={
    RUNPOD_COSYVOICE_TTS_URL=https://api.runpod.ai/v2/${RUNPOD_ENDPOINT_ID}/runsync
  }"
```

## API使用例

### SFTモード（プリセット音声）
```bash
curl -X POST https://api.runpod.ai/v2/YOUR_ENDPOINT_ID/runsync \
  -H "Authorization: Bearer YOUR_RUNPOD_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "input": {
      "text": "こんにちは、ゆうきです。",
      "mode": "sft",
      "speaker_id": "中文女",
      "output_format": "mp3"
    }
  }'
```

### Zero-shotモード（音声クローニング）
```bash
# リファレンス音声をbase64エンコード
REFERENCE_AUDIO=$(base64 -i yuki_voice.wav | tr -d '\n')

curl -X POST https://api.runpod.ai/v2/YOUR_ENDPOINT_ID/runsync \
  -H "Authorization: Bearer YOUR_RUNPOD_API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"input\": {
      \"text\": \"chatweb.aiへようこそ！\",
      \"mode\": \"zero_shot\",
      \"prompt_audio\": \"$REFERENCE_AUDIO\",
      \"prompt_text\": \"こんにちは\",
      \"output_format\": \"mp3\"
    }
  }"
```

### Instructモード（スタイル制御）
```bash
curl -X POST https://api.runpod.ai/v2/YOUR_ENDPOINT_ID/runsync \
  -H "Authorization: Bearer YOUR_RUNPOD_API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"input\": {
      \"text\": \"これは重要なお知らせです。\",
      \"mode\": \"instruct\",
      \"prompt_audio\": \"$REFERENCE_AUDIO\",
      \"instruct_text\": \"真剣なトーンで、ゆっくりと話してください。\",
      \"output_format\": \"mp3\"
    }
  }"
```

## モデル情報
- **モデル**: CosyVoice2-0.5B
- **サイズ**: ~2GB
- **VRAM**: 4-8GB (推論時)
- **レイテンシ**: 1-3秒 (テキスト長に依存)

## 利用可能な話者

### 中国語
- 中文女, 中文男, 粤语女, 东北老妹儿, 广西大妹砸

### 英語
- English Female, English Male

### 日本語
- 日语男

### 韓国語
- 韩语女

## コスト見積もり
- **A10 GPU**: $0.0004/秒 ≈ $1.44/時間
- **アイドル時**: $0 (オートスケール)
- **平均リクエスト**: 2秒 ≈ $0.0008/リクエスト

## トラブルシューティング

### OOM (Out of Memory)
- GPU を A100 (40GB) にアップグレード
- Container Disk を増やす

### モデルロードエラー
- `MODEL_DIR`環境変数を確認
- モデルキャッシュが正しくダウンロードされているか確認

### 音声品質が低い
- `mode="zero_shot"` を使用
- リファレンス音声の品質を改善（ノイズ除去、正規化）
- プロンプトテキストを正確に入力
