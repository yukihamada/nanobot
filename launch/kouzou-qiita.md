---
title: "Next.js 16 + React 19 で木造住宅の耐震診断Webアプリを作った"
tags: Next.js, React, TypeScript, 耐震, 建築
---

## はじめに

日本は地震大国です。木造住宅に住んでいる方なら「うちの家、大地震が来たら大丈夫だろうか」と一度は考えたことがあるのではないでしょうか。

しかし、専門家による耐震診断は **費用が10〜50万円**、結果が出るまで **数週間〜数ヶ月** かかるのが実情です。自治体の補助金制度はあるものの、申請手続きのハードルもあり、「まず自分の家がどの程度危ないのか」を気軽に知る手段がありませんでした。

そこで、日本建築防災協会の「木造住宅の耐震診断と補強方法（2012年改訂版）」に準拠した**耐震診断Webアプリ「耐震診断くん」**を開発しました。ブラウザだけで完結し、サーバーにデータを送信しないため、プライバシーも安心です。

**試してみる**: https://kouzou.fly.dev

## 技術スタック

| カテゴリ | 技術 | 選定理由 |
|----------|------|----------|
| フレームワーク | **Next.js 16** (App Router) | RSC + Streaming で初期表示が速い |
| UI | **React 19** + **Tailwind CSS 4** + shadcn/ui | サーバーコンポーネントとの親和性、ユーティリティファーストCSS |
| 状態管理 | **Zustand** | ウィザード形式の複数ステップで状態を保持。Redux より軽量 |
| フォーム | React Hook Form + **Zod** | バリデーションスキーマを型と共有 |
| PDF生成 | **jsPDF** | クライアントサイドで診断レポートPDFを生成 |
| グラフ | Recharts | 壁量充足率などの可視化 |
| テスト | Vitest + React Testing Library | 87テスト、カバレッジ約90% |
| デプロイ | **Fly.io** 東京リージョン | Dockerマルチステージビルドで軽量イメージ |

ポイントは**全ての計算がブラウザ内で完結する**ことです。構造計算は純粋なTypeScript関数として実装しているため、サーバーサイドのAPIは不要です。ヘルスチェック用の `/api/health` 以外にAPIルートはありません。

## アプリの構成

診断には2つのモードがあります。

- **簡易診断（10問・約5分）**: 一般の住宅所有者向け。「誰でもできるわが家の耐震診断」に準拠
- **精密診断（一般診断法）**: 建築士向け。上部構造評点（Iw値）を算出し、PDFレポートを出力

精密診断の計算ロジックが技術的に面白いので、以下で詳しく解説します。

## 計算ロジックの解説

### 上部構造評点（Iw値）― メインの診断指標

Iw値は木造住宅の耐震性能を表す最も重要な指標です。計算式は次のとおりです。

```
Iw = edQu / Qr

edQu = Qu × eKfl × dK
```

| 記号 | 意味 |
|------|------|
| Qu | 保有耐力（壁の耐力の合計） |
| Qr | 必要耐力（建物が地震に耐えるために必要な力） |
| eKfl | 偏心率による低減係数（壁の配置バランス） |
| dK | 劣化低減係数（経年劣化の影響） |

結果は4段階で評価されます。

| Iw値 | 判定 |
|------|------|
| 1.5以上 | 倒壊しない |
| 1.0〜1.5 | 一応倒壊しない |
| 0.7〜1.0 | 倒壊する可能性がある |
| 0.7未満 | 倒壊する可能性が高い |

TypeScriptでの実装はこのようになっています。

```typescript
// src/lib/calc/upper-structure-score.ts

export function calculateDetailedDiagnosis(
  buildingInfo: BuildingInfo,
  walls: WallSegment[],
  deteriorationItems: DeteriorationItem[]
): DetailedDiagnosisResult {
  const deteriorationScore = calculateDeteriorationFactor(deteriorationItems)
  const directions: WallDirection[] = ['X', 'Y']
  const floors: (1 | 2)[] =
    buildingInfo.numberOfFloors >= 2 ? [1, 2] : [1]

  const directionalResults: DirectionalResult[] = []

  for (const floor of floors) {
    for (const direction of directions) {
      const qu = calculateWallStrengthSum(walls, floor, direction)
      const qr = calculateRequiredCapacity(floor, buildingInfo)

      // 偏心率の計算
      const ecResult = calculateEccentricity(walls, floorShape, floor, direction)
      const eccentricityFactor = ecResult.correctionFactor

      // Iw値の算出
      const dK = deteriorationScore.dK
      const edQu = qu * eccentricityFactor * dK
      const iw = qr > 0 ? edQu / qr : 0

      directionalResults.push({ floor, direction, qu, qr, edQu, iw, /* ... */ })
    }
  }

  // 全方向・全階の最小値が建物全体の評点
  const overallIw = Math.min(...directionalResults.map((r) => r.iw))
  return { overallIw, overallRating: getStructuralRating(overallIw), /* ... */ }
}
```

各階・各方向（X方向・Y方向）について個別にIw値を算出し、**最も弱い方向の値が建物全体の評点**になるのがポイントです。地震はどの方向から来るか分からないため、最弱方向で評価します。

### 壁量計算 ― 保有耐力 Qu

壁1枚あたりの耐力は `Qw = Fw × L × Kj` で計算します。

```typescript
// src/lib/calc/wall-strength.ts

export function calculateSingleWallStrength(wall: WallSegment): number {
  if (wall.wallType === 'none') return 0

  const baseStrength = getBaseStrength(wall)  // Fw: 壁基準耐力 (kN/m)
  const kj = JOINT_REDUCTION_FACTORS[wall.jointSpec]  // Kj: 接合部低減係数
  let strength = baseStrength * wall.length * kj

  // 裏面仕上げがある場合はその耐力も加算
  if (wall.backSurface && wall.backSurface !== 'none') {
    const backStrength = WALL_BASE_STRENGTH[wall.backSurface]
    strength += backStrength * wall.length * kj
  }

  return strength
}
```

壁の種類（筋かい、構造用合板など）ごとに基準耐力 `Fw` が定数テーブルで定義されています。接合部の仕様（金物あり/なし）によって低減係数 `Kj` が変わるのも現実の構造計算と同じです。

### 必要耐力 Qr

建物が地震に耐えるために必要な力を、略算法で求めます。

```typescript
// src/lib/calc/required-capacity.ts

// Qr = C × A × Z × Sg
export function calculateRequiredCapacity(
  targetFloor: 1 | 2 | 3,
  buildingInfo: BuildingInfo
): number {
  const coefficient = getRequiredCapacityCoefficient(
    numberOfFloors, targetFloor, roofWeight
  )  // C: 必要耐力係数
  const area = getFloorArea(targetFloor, floorAreas)  // A: 床面積
  const groundFactor = GROUND_FACTORS[groundType]  // Sg: 地盤割増係数
  const z = regionCoefficientZ  // Z: 地域係数

  return coefficient * area * z * groundFactor
}
```

階数、屋根の重さ（瓦 vs スレート）、地盤の種類、地域係数（沖縄は0.7、東京は1.0など）を考慮して必要耐力を算出します。

### 劣化低減係数 dK

建物の経年劣化を数値化します。シロアリ被害、雨漏り、基礎のひび割れなどの劣化項目をチェックし、劣化度に応じてIw値を下げます。

```typescript
// src/lib/calc/deterioration.ts

export function calculateDeteriorationFactor(
  items: DeteriorationItem[]
): DeteriorationResult {
  const existingItems = items.filter((item) => item.exists)
  const totalExistencePoints = existingItems.reduce((sum, item) => sum + item.points, 0)
  const totalDeteriorationPoints = existingItems
    .filter((item) => item.checked)
    .reduce((sum, item) => sum + item.points, 0)

  // dK = 1 - (劣化点数 / 存在点数)、最低0.7
  let dK = totalExistencePoints === 0
    ? 1.0
    : Math.max(0.7, 1 - totalDeteriorationPoints / totalExistencePoints)

  return { dK, totalDeteriorationPoints, totalExistencePoints, items }
}
```

最低値が **0.7** に制限されているのは、劣化が著しい場合でも構造体がゼロにはならないという考え方に基づいています。

### 偏心率 ― 壁の配置バランス

壁が片側に偏って配置されていると、地震時にねじれが発生して倒壊しやすくなります。偏心率はこのバランスを定量化する指標です。

```typescript
// src/lib/calc/eccentricity.ts

export function calculateEccentricity(
  walls: WallSegment[], floorShape: FloorShape,
  floor: 1 | 2, evaluationDirection: WallDirection
): EccentricityResult {
  // 重心: 平面形状の幾何学的中心
  const centerOfGravity = floorDimension / 2

  // 剛心: 壁耐力の重心位置
  const centerOfRigidity =
    wallData.reduce((s, w) => s + w.strength * w.position, 0) / totalStrength

  // 偏心距離 → ねじり剛性 → 弾力半径 → 偏心率
  const eccentricityDistance = Math.abs(centerOfGravity - centerOfRigidity)
  const elasticRadius = Math.sqrt(torsionalRigidity / totalStrength)
  const ratio = elasticRadius > 0 ? eccentricityDistance / elasticRadius : 1.0

  return { ratio, correctionFactor: getEccentricityCorrectionFactor(ratio) }
}
```

偏心率が0.15以下なら低減なし（係数1.0）、0.45以上で最大低減（係数0.5）。この間は線形補間で滑らかに変化します。壁を均等に配置することがいかに重要か、数値で実感できます。

## 状態管理の設計

ウィザード形式のフォーム（建物情報 → 壁データ入力 → 劣化チェック → 結果表示）を **Zustand** で管理しています。

```
src/stores/
├── detailed-diagnosis-store.ts   # 精密診断のウィザード状態
└── simple-diagnosis-store.ts     # 簡易診断の回答状態
```

Zustandを選んだ理由は、ボイラープレートが少なく、コンポーネント外（計算ロジック側）からも状態にアクセスしやすいためです。Redux DevToolsとの連携も可能なので、デバッグ時に各ステップの状態遷移を追いやすい利点もあります。

## テスト戦略

構造計算の正確性は建物の安全に関わるため、**計算ロジックのテストを最優先**にしました。

- **87テスト、カバレッジ約90%**
- 壁量計算、必要耐力、偏心率、劣化度、上部構造評点のそれぞれに個別テスト
- 既知の構造計算例（教科書の演習問題）と突合して検証

```bash
npm run test           # Vitest で全テスト実行
npm run test:coverage  # カバレッジレポート
```

計算ロジックを純粋関数として切り出しているため、UIに依存せずテストできます。これはReact Testing Libraryでコンポーネントテストを書くよりはるかに高速で安定します。

## デプロイ

Fly.ioの東京リージョンにDockerマルチステージビルドでデプロイしています。Next.jsのstandaloneモードを使い、イメージサイズを最小限に抑えています。

```bash
fly deploy  # これだけ
```

サーバーサイドの計算がないため、インスタンスのスペックは最小限で済みます。

## まとめ

- 木造住宅の耐震診断をブラウザだけで完結するWebアプリを作った
- 2012年改訂版の一般診断法に準拠し、Iw値・壁量・偏心率・劣化度を正しく計算
- 計算ロジックは純粋なTypeScript関数として実装し、テストカバレッジ90%で品質を担保
- Next.js 16 + React 19の最新スタックで、SSR + クライアントサイド計算のハイブリッド構成

**免責事項**: 本ツールは簡易的な参考情報を提供するものです。正確な耐震診断には建築士等の専門家による現地調査が必要です。

ぜひ一度お試しください: **https://kouzou.fly.dev**

ソースコードはMITライセンスで公開しています: https://github.com/yukihamada/kouzou
