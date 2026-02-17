# Qwen3 Voice Cloning TTS for RunPod

Alibaba Qwen3-Audio-Chat モデルを使った高品質な音声合成と音声クローニング。

## 特徴
- **音声クローニング**: 5-10秒のリファレンス音声から声を再現
- **多言語対応**: 日本語、英語、中国語
- **高品質**: 24kHz サンプリングレート
- **速度調整**: 0.5x - 2.0x の速度変更

## デプロイ

### 1. Dockerイメージをビルド
```bash
cd infra/runpod/qwen-tts
docker build -t YOUR_DOCKERHUB/qwen-tts:latest .
docker push YOUR_DOCKERHUB/qwen-tts:latest
```

### 2. RunPodでデプロイ
1. RunPod Console → Serverless → Endpoints
2. New Template:
   - **Container Image**: `YOUR_DOCKERHUB/qwen-tts:latest`
   - **Container Disk**: 20GB
   - **GPU**: A10 (24GB) または A100
3. New Endpoint:
   - **Template**: 上記で作成したテンプレート
   - **Active Workers**: 0-1 (オートスケール)
   - **Max Workers**: 3

### 3. Lambda環境変数に追加
```bash
# Endpoint URLを取得（RunPod Console → Endpoint → API）
RUNPOD_ENDPOINT_ID="your_endpoint_id"

aws lambda update-function-configuration \
  --function-name nanobot \
  --region ap-northeast-1 \
  --environment "Variables={
    RUNPOD_QWEN_TTS_URL=https://api.runpod.ai/v2/${RUNPOD_ENDPOINT_ID}/runsync
  }"
```

## API使用例

### 基本的な音声合成
```bash
curl -X POST https://api.runpod.ai/v2/YOUR_ENDPOINT_ID/runsync \
  -H "Authorization: Bearer YOUR_RUNPOD_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "input": {
      "text": "こんにちは、ゆうきです。",
      "language": "ja",
      "speed": 1.0
    }
  }'
```

### 音声クローニング
```bash
# 1. リファレンス音声をbase64エンコード
REFERENCE_AUDIO=$(base64 -i yuki_voice.wav | tr -d '\n')

# 2. クローニング合成
curl -X POST https://api.runpod.ai/v2/YOUR_ENDPOINT_ID/runsync \
  -H "Authorization: Bearer YOUR_RUNPOD_API_KEY" \
  -H "Content-Type: application/json" \
  -d "{
    \"input\": {
      \"text\": \"chatweb.aiへようこそ！\",
      \"voice\": \"clone\",
      \"reference_audio\": \"$REFERENCE_AUDIO\",
      \"language\": \"ja\"
    }
  }"
```

## モデル情報
- **モデル**: Qwen/Qwen-Audio-Chat
- **サイズ**: ~7GB
- **VRAM**: 10-15GB (推論時)
- **レイテンシ**: 1-3秒 (テキスト長に依存)

## コスト見積もり
- **A10 GPU**: $0.0004/秒 ≈ $1.44/時間
- **アイドル時**: $0 (オートスケール)
- **平均リクエスト**: 2秒 ≈ $0.0008/リクエスト

## トラブルシューティング

### OOM (Out of Memory)
- GPU を A100 (40GB) にアップグレード
- バッチサイズを減らす

### レイテンシが高い
- Keep Warm を 1 に設定
- より高速なGPU (A100) を使用

### 音声品質が低い
- サンプリングレートを確認（24kHz推奨）
- リファレンス音声の品質を改善（ノイズ除去、正規化）
