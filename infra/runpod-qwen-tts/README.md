# Qwen3 Voice Cloning on RunPod

RunPodにデプロイ可能なQwen3ボイスクローニングサービス。高品質な音声合成とカスタム音声クローニングをサポート。

## Features

- 🎤 **Voice Cloning**: 参照音声から声をクローン
- 🌏 **多言語対応**: 日本語・英語を自動検出
- ⚡ **GPU加速**: CUDA対応で高速生成
- 🔌 **REST API**: シンプルなHTTPエンドポイント
- 📦 **Docker対応**: すぐにデプロイ可能

## Quick Start

### 1. RunPodにデプロイ

```bash
cd /Users/yuki/workspace/nanobot/infra/runpod-qwen-tts

# RunPod APIキーを設定
export RUNPOD_API_KEY="your_runpod_api_key_here"

# デプロイ実行
./deploy-runpod.sh
```

### 2. エンドポイントURLを設定

デプロイ完了後、RunPodダッシュボードからエンドポイントURLを取得:

```bash
export RUNPOD_QWEN_TTS_URL="https://your-pod-id-8000.proxy.runpod.net"
```

### 3. chatweb.aiから使用

```bash
# nanobotサーバーを再起動
cd /Users/yuki/workspace/nanobot
cargo build --release
./target/release/nanobot gateway --http --http-port 3000
```

## API Usage

### 基本的な音声合成

```bash
curl -X POST "http://localhost:3000/api/v1/speech/synthesize" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "こんにちは、世界！",
    "engine": "runpod-qwen",
    "voice": "default",
    "speed": 1.0
  }' \
  --output speech.wav
```

### ボイスクローニング

```bash
# 1. 参照音声をBase64エンコード
REFERENCE_AUDIO=$(base64 -i reference_voice.wav)

# 2. クローン音声で合成
curl -X POST "http://localhost:3000/api/v1/speech/synthesize" \
  -H "Content-Type: application/json" \
  -d "{
    \"text\": \"こんにちは、私はあなたの声のクローンです。\",
    \"engine\": \"runpod-qwen\",
    \"reference_audio\": \"$REFERENCE_AUDIO\",
    \"speed\": 1.0
  }" \
  --output cloned_speech.wav
```

### 直接RunPodエンドポイントを使用

```bash
curl -X POST "$RUNPOD_QWEN_TTS_URL/synthesize" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Hello, world!",
    "voice": "default",
    "language": "en",
    "speed": 1.0
  }' | jq '.audio' -r | base64 -d > output.wav
```

## Configuration

### 環境変数

| 変数名 | 説明 | 必須 |
|--------|------|------|
| `RUNPOD_API_KEY` | RunPod APIキー（デプロイ時） | ✅ |
| `RUNPOD_QWEN_TTS_URL` | デプロイされたエンドポイントURL | ✅ |
| `MODEL_NAME` | 使用するHugging Faceモデル | ❌ (デフォルト: Qwen/Qwen2.5-TTS) |

### サポートされているパラメータ

- **text** (必須): 合成するテキスト
- **voice** (オプション): 音声プリセット名 (デフォルト: "default")
- **language** (オプション): 言語コード "ja" / "en" / "auto" (デフォルト: auto)
- **speed** (オプション): 再生速度 0.5-2.0 (デフォルト: 1.0)
- **reference_audio** (オプション): Base64エンコードされた参照音声（ボイスクローン用）

## Development

### ローカルテスト

```bash
cd infra/runpod-qwen-tts

# Dockerイメージをビルド
docker build -t qwen3-voice-clone .

# ローカルで実行（CPU）
docker run -p 8000:8000 \
  -e MODEL_NAME="Qwen/Qwen2.5-TTS" \
  qwen3-voice-clone

# GPUで実行
docker run --gpus all -p 8000:8000 \
  -e MODEL_NAME="Qwen/Qwen2.5-TTS" \
  qwen3-voice-clone
```

### ヘルスチェック

```bash
curl http://localhost:8000/health
```

### API ドキュメント

ブラウザで開く: `http://localhost:8000/docs`

## Performance

### 推奨GPU

- **最小**: NVIDIA RTX A4000 (16GB VRAM)
- **推奨**: NVIDIA A40 / A100 (40GB+ VRAM)
- **最適**: NVIDIA H100 (80GB VRAM)

### 生成速度

- **短文 (~20文字)**: 1-2秒
- **中文 (~100文字)**: 3-5秒
- **長文 (~500文字)**: 10-15秒

※GPU・テキスト長により変動

## Troubleshooting

### モデルが読み込めない

```bash
# モデル名を確認
echo $MODEL_NAME

# Hugging Faceから手動でダウンロード
python -c "from transformers import AutoModel; AutoModel.from_pretrained('Qwen/Qwen2.5-TTS')"
```

### 音声が生成されない

- RunPodのログを確認: `https://runpod.io/console/pods`
- エンドポイントURLが正しいか確認
- APIキーの権限を確認

### 音質が悪い

- 参照音声の品質を向上（16kHz以上、モノラル推奨）
- `speed`パラメータを調整
- より大きいモデルに変更

## Cost Estimation

RunPod料金（目安）:

- **NVIDIA RTX A4000**: $0.29/hr
- **NVIDIA A40**: $0.79/hr
- **NVIDIA A100 (40GB)**: $1.89/hr

1000回の音声合成（平均5秒/回）:
- 処理時間: 約1.5時間
- コスト: $0.44 - $2.84（GPU次第）

## License

このプロジェクトはMITライセンスの下で公開されています。

## Support

問題が発生した場合:
1. [Issues](https://github.com/yukihamada/nanobot/issues)に報告
2. RunPodドキュメント: https://docs.runpod.io
3. Qwen TTSドキュメント: https://huggingface.co/Qwen
