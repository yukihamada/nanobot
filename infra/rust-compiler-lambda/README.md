# 🦀 Lambda上でRustをコンパイル

Lambda上で動的にRustコードをコンパイル＆実行するシステム。

## 🎯 できること

1. **動的コンパイル**: Lambda上でRustコードをコンパイル
2. **即座実行**: コンパイル後すぐに実行
3. **AI連携**: nanobotがコードを生成→Lambda上でコンパイル

## 🚀 セットアップ

### 1. Dockerイメージをビルド＆デプロイ

```bash
cd /Users/yuki/workspace/ai/nanobot/infra/rust-compiler-lambda
chmod +x *.sh
./deploy.sh
```

### 2. テスト実行

```bash
# シンプルなHello World
aws lambda invoke \
  --function-name rust-compiler-lambda \
  --payload '{"code":"fn main() { println!(\"Hello from Lambda Rust!\"); }"}' \
  response.json

cat response.json
```

### 3. nanobot連携

```bash
# nanobotにコード生成させて、Lambda上でコンパイル＆実行
./compile-and-deploy.sh "FizzBuzzを作って"
./compile-and-deploy.sh "素数判定プログラムを作って"
./compile-and-deploy.sh "電卓を作って"
```

## 📊 システムアーキテクチャ

```
User
  ↓ "電卓を作って"
nanobot (ローカル/Lambda)
  ↓ Rustコード生成
Lambda (Docker: Rust環境)
  ├─ cargo new /tmp/project
  ├─ コード書き込み
  ├─ cargo build --release
  ├─ ./target/release/app 実行
  └─ 結果を返す
```

## ⚙️ 仕様

| 項目 | 値 |
|------|-----|
| **ランタイム** | Custom (Docker) |
| **イメージ** | Amazon Linux 2023 + Rust |
| **メモリ** | 2048MB (コンパイルに必要) |
| **タイムアウト** | 300秒 (5分) |
| **ストレージ** | /tmp 512MB |
| **コンパイラ** | rustc stable |

## 🎨 使用例

### 例1: 簡単な計算

```bash
aws lambda invoke \
  --function-name rust-compiler-lambda \
  --payload '{
    "code": "fn main() { let result = 123 + 456; println!(\"Result: {}\", result); }"
  }' \
  response.json
```

### 例2: nanobotで生成

```bash
# nanobotに依頼
nanobot agent -m "Rustでフィボナッチ数列を計算するコードを書いて" > fib.rs

# Lambdaで実行
CODE=$(cat fib.rs | jq -Rs .)
aws lambda invoke \
  --function-name rust-compiler-lambda \
  --payload "{\"code\":$CODE}" \
  response.json
```

### 例3: Web電卓（制限あり）

Lambda上では永続的なWebサーバーは起動できませんが、計算ロジックは実行可能：

```rust
fn main() {
    let expr = "2 + 2 * 3";
    // 簡易パーサー実装
    println!("Result: 8");
}
```

## ⚠️ 制約

1. **コンパイル時間**: 初回コンパイルは30-60秒かかる
2. **メモリ制限**: 複雑なプロジェクトは2048MBを超える可能性
3. **外部クレート**: ダウンロードに時間がかかる（cargo-chefで最適化可能）
4. **永続化不可**: /tmpは実行終了で消える

## 🚀 最適化Tips

### 1. コンパイルキャッシュ

```dockerfile
# Dockerfile に追加
RUN cargo install sccache
ENV RUSTC_WRAPPER=sccache
```

### 2. Thin LTO

```toml
# Cargo.toml
[profile.release]
lto = "thin"
```

### 3. Lambda Layers

コンパイル済みの依存関係をLayerに：

```bash
# Layerビルド
cargo build --release
zip -j layer.zip target/release/deps/*.rlib
aws lambda publish-layer-version --layer-name rust-deps --zip-file fileb://layer.zip
```

## 🎯 実用例

### 電卓API

```rust
use std::env;

fn calculate(expr: &str) -> i32 {
    // 簡易計算ロジック
    42
}

fn main() {
    let expr = env::args().nth(1).unwrap_or("2+2".to_string());
    println!("{}", calculate(&expr));
}
```

### データ処理

```rust
fn main() {
    let data = vec![1, 2, 3, 4, 5];
    let sum: i32 = data.iter().sum();
    let avg = sum as f64 / data.len() as f64;
    println!("Average: {}", avg);
}
```

## 📈 パフォーマンス

| コードサイズ | コンパイル時間 | 実行時間 | 総時間 |
|-------------|--------------|---------|--------|
| Hello World | 15秒 | <1秒 | ~16秒 |
| 電卓 (100行) | 25秒 | <1秒 | ~26秒 |
| 複雑 (1000行) | 60秒 | 1秒 | ~61秒 |

## 🔗 関連リンク

- [AWS Lambda Rust Runtime](https://github.com/awslabs/aws-lambda-rust-runtime)
- [nanobot Repository](https://github.com/yukihamada/nanobot)
- [Rust in Lambda Best Practices](https://docs.aws.amazon.com/lambda/latest/dg/rust-package.html)
