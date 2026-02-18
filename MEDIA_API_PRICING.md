# Media API 完全ガイド & 価格表

## 📊 全エンドポイント一覧

### 🎤 音声系

#### TTS (音声合成)
```bash
POST /api/v1/media/tts
```
**価格:** 1 credit / 100文字（最低1 credit）
**エンジン:** OpenAI / ElevenLabs / Polly（自動フォールバック）
**品質:** 高品質Neural Voice対応

#### STT (音声認識)
```bash
POST /api/v1/media/stt
```
**価格:** TBD (未実装)
**エンジン:** Whisper API

#### 効果音生成 NEW! 🔊
```bash
POST /api/v1/media/sfx
{
  "prompt": "footsteps on wooden floor",
  "duration": 3
}
```
**価格:**
- 3秒: 5 credits
- 10秒: 17 credits

**使用例:**
- ゲーム効果音
- 動画SE
- ポッドキャスト素材

---

### 🎵 音楽系

#### 音楽生成
```bash
POST /api/v1/media/music
{
  "prompt": "アコースティックギターのカフェBGM",
  "type": "music",
  "duration": 30
}
```
**価格:**
- 10秒: 10 credits
- 30秒: 20 credits
- 60秒: 40 credits

**プロバイダー:** Stable Audio（高品質）

---

### 🖼️ 画像系

#### 画像生成
```bash
POST /api/v1/media/image
{
  "prompt": "a cat in the snow, cinematic",
  "model": "dalle-3",
  "quality": "standard"
}
```
**価格:**
| モデル | 品質 | クレジット |
|--------|------|-----------|
| DALL-E 3 | HD | 20 |
| DALL-E 3 | standard | 10 |
| Flux Pro | - | 15 |
| Flux Realism | - | 5 |
| Flux Schnell | - | 5 |

#### 背景削除 NEW! 🎨
```bash
POST /api/v1/media/remove-bg
{
  "image_url": "https://example.com/image.jpg",
  "quality": "standard"
}
```
**価格:**
- standard: 8 credits
- HD: 15 credits

**モデル:** BRIA RMBG（最先端AI）

**使用例:**
- ECサイト商品画像
- プロフィール写真加工
- デザイン素材作成

#### アップスケール NEW! 📈
```bash
POST /api/v1/media/upscale
{
  "image_url": "https://example.com/low-res.jpg",
  "scale": 2,
  "model": "fast"
}
```
**価格:**
| Scale | Model | クレジット |
|-------|-------|-----------|
| 2x | fast | 12 |
| 2x | quality | 15 |
| 4x | fast | 20 |
| 4x | quality | 25 |

**モデル:** Real-ESRGAN (fast) / CCSR (quality)

**使用例:**
- 低解像度画像の改善
- 印刷用高品質化
- AI生成画像の精細化

#### OCR (文字認識) NEW! 🔍
```bash
POST /api/v1/media/ocr
{
  "image_url": "https://example.com/document.jpg",
  "language": "ja",
  "format": "text"
}
```
**価格:**
- standard (Tesseract): 5 credits
- premium (Google Vision): 10 credits

**対応言語:** 日本語、英語、中国語、韓国語など100+言語

**使用例:**
- レシート読み取り
- 名刺データ化
- ドキュメントデジタル化

---

### 🎬 動画系

#### 動画生成
```bash
POST /api/v1/media/video
{
  "prompt": "a dog running on beach at sunset",
  "duration": 5,
  "mode": "standard"
}
```
**価格:**
| 長さ | モード | クレジット |
|------|--------|-----------|
| 5秒 | standard | 50 |
| 10秒 | standard | 100 |
| 5秒 | pro | 150 |
| 10秒 | pro | 300 |

**プロバイダー:** Kling AI（非同期処理）

---

## 🎯 推奨ユースケース別価格

### ウェブアプリ開発者向け
```
OCR (5) + Remove BG (8) + Upscale (12) = 25 credits
→ 商品画像の完全処理パイプライン
```

### コンテンツクリエイター向け
```
Image (10) + SFX (5) + Music (20) = 35 credits
→ SNS投稿用動画素材一式
```

### ビジネスユース
```
TTS (1/100字) + OCR (5) + Image (10) = 16+ credits
→ プレゼン資料自動生成
```

---

## 💰 プラン別月額クレジット

| プラン | 月額 | 月間クレジット | 1 creditあたり |
|--------|------|----------------|----------------|
| Free | ¥0 | 100 | - |
| Starter | ¥980 | 1,000 | ¥0.98 |
| Pro | ¥2,980 | 5,000 | ¥0.60 |
| Business | ¥9,800 | 20,000 | ¥0.49 |

---

## 🚀 高品質モード（裏メニュー）

すべてのエンドポイントで、より高性能なモデルを使用可能：

### 現在利用可能
- **OCR**: `language: "premium"` → Google Cloud Vision (10 credits)
- **Remove BG**: `quality: "hd"` → BRIA RMBG v2 (15 credits)
- **Upscale**: `model: "quality"` → CCSR (高品質モデル)
- **Image**: `model: "flux-pro"` → Flux Pro (15 credits)
- **Video**: `mode: "pro"` → Kling AI Pro (150-300 credits)

### 今後追加予定
- **3D生成**: Meshy AI / Luma Dream Machine
- **音源分離**: Spleeter Pro / Demucs v4
- **ボイスクローン**: ElevenLabs Voice Lab
- **モデレーション**: OpenAI Moderation API

---

## 📝 使用例コード

### JavaScript
```javascript
const response = await fetch('https://api.chatweb.ai/api/v1/media/remove-bg', {
  method: 'POST',
  headers: {
    'Authorization': `Bearer ${token}`,
    'Content-Type': 'application/json'
  },
  body: JSON.stringify({
    image_url: 'https://example.com/photo.jpg',
    quality: 'standard'
  })
});

const { url, credits_used } = await response.json();
console.log('Background removed:', url);
console.log('Credits used:', credits_used);
```

### Python
```python
import requests

response = requests.post(
    'https://api.chatweb.ai/api/v1/media/ocr',
    headers={'Authorization': f'Bearer {token}'},
    json={
        'image_url': 'https://example.com/receipt.jpg',
        'language': 'ja',
        'format': 'text'
    }
)

data = response.json()
print(f"Recognized text: {data['text']}")
print(f"Confidence: {data['confidence']}")
print(f"Credits used: {data['credits_used']}")
```

### cURL
```bash
curl -X POST https://api.chatweb.ai/api/v1/media/upscale \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "image_url": "https://example.com/low-res.jpg",
    "scale": 4,
    "model": "quality"
  }'
```

---

## 🔒 セキュリティ & レート制限

- **認証:** Bearer token必須（全エンドポイント）
- **レート制限:**
  - Free: 10 requests/min
  - Starter: 30 requests/min
  - Pro: 100 requests/min
- **最大ファイルサイズ:**
  - 画像: 10MB
  - 音声: 25MB
  - 動画: N/A（生成のみ）
- **タイムアウト:**
  - 軽量処理: 60秒
  - 画像生成: 120秒
  - 動画生成: 非同期（最大10分）

---

## 📊 統計・分析

すべてのAPI使用は以下で追跡可能：
- `GET /api/v1/auth/me` - 残クレジット確認
- `GET /api/v1/usage` - 使用履歴（実装予定）
- `GET /api/v1/analytics` - コスト分析（実装予定）

---

**最終更新:** 2026-02-18
**APIバージョン:** v0.2.0
**ドキュメント:** https://api.chatweb.ai/docs
