# Deployment Guide

This guide covers deploying nanobot to various platforms.

## AWS Lambda (Recommended for Production)

### Prerequisites
- AWS CLI configured with credentials
- AWS SAM CLI installed
- Rust 1.75+
- Zig (for cross-compilation)

### Step 1: Cross-Compile for ARM64

```bash
# Install cross-compilation tools
brew install zig
cargo install cargo-zigbuild
rustup target add aarch64-unknown-linux-gnu

# Build for Lambda ARM64
RUSTUP_TOOLCHAIN=stable \
RUSTC=~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc \
cargo zigbuild \
  --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu
```

### Step 2: Deploy with SAM

```bash
cd infra

# First-time deployment (guided)
sam build && sam deploy --guided

# Subsequent deployments
sam build && sam deploy
```

### Step 3: Configure Environment Variables

```bash
# Set via AWS Console or CLI
aws lambda update-function-configuration \
  --function-name nanobot \
  --environment Variables="{
    OPENAI_API_KEY=sk-...,
    ANTHROPIC_API_KEY=sk-ant-...,
    DYNAMODB_TABLE=nanobot-table,
    BASE_URL=https://chatweb.ai
  }"
```

### Step 4: Set up DynamoDB

The SAM template automatically creates a DynamoDB table with:
- On-demand billing
- Point-in-time recovery
- Encryption at rest

### Step 5: Configure API Gateway

The SAM template creates:
- HTTP API Gateway (v2)
- Custom domain (optional)
- CORS configuration

### Fast Deployment (Without SAM)

For quick updates without SAM:

```bash
# Build
cargo zigbuild --manifest-path crates/nanobot-lambda/Cargo.toml \
  --release --target aarch64-unknown-linux-gnu

# Package
cd target/aarch64-unknown-linux-gnu/release
zip -j /tmp/nanobot-lambda.zip bootstrap

# Deploy
aws lambda update-function-code \
  --function-name nanobot \
  --zip-file "fileb:///tmp/nanobot-lambda.zip" \
  --region ap-northeast-1

# Publish version
aws lambda publish-version \
  --function-name nanobot \
  --region ap-northeast-1

# Update alias
aws lambda update-alias \
  --function-name nanobot \
  --name live \
  --function-version $LATEST_VERSION \
  --region ap-northeast-1
```

---

## Docker (Self-Hosted)

### Docker Compose (Recommended)

```yaml
# docker-compose.yml
version: '3.8'

services:
  nanobot:
    image: ghcr.io/yukihamada/nanobot:latest
    ports:
      - "3000:3000"
    environment:
      - OPENAI_API_KEY=sk-...
      - ANTHROPIC_API_KEY=sk-ant-...
      - BASE_URL=https://your-domain.com
    volumes:
      - ./data:/data
    restart: unless-stopped
```

```bash
docker compose up -d
```

### Docker Run

```bash
docker run -d \
  --name nanobot \
  -p 3000:3000 \
  -e OPENAI_API_KEY=sk-... \
  -e ANTHROPIC_API_KEY=sk-ant-... \
  -v $(pwd)/data:/data \
  --restart unless-stopped \
  ghcr.io/yukihamada/nanobot:latest
```

### Build Your Own Docker Image

```dockerfile
FROM rust:1.75-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/nanobot /usr/local/bin/
CMD ["nanobot", "gateway", "--http", "--http-port", "3000"]
```

```bash
docker build -t nanobot:custom .
docker run -p 3000:3000 nanobot:custom
```

---

## Fly.io

### Prerequisites
- Fly.io account
- `flyctl` CLI installed

### Deploy

```bash
# Login
flyctl auth login

# Create app
flyctl launch --name nanobot

# Set secrets
flyctl secrets set \
  OPENAI_API_KEY=sk-... \
  ANTHROPIC_API_KEY=sk-ant-...

# Deploy
flyctl deploy

# Scale (optional)
flyctl scale vm shared-cpu-1x --memory 512
```

### fly.toml

```toml
app = "nanobot"

[build]
  dockerfile = "Dockerfile"

[env]
  PORT = "8080"

[[services]]
  internal_port = 8080
  protocol = "tcp"

  [[services.ports]]
    handlers = ["http"]
    port = 80

  [[services.ports]]
    handlers = ["tls", "http"]
    port = 443
```

---

## Railway

### Deploy with One Click

[![Deploy on Railway](https://railway.app/button.svg)](https://railway.app/new/template?template=https://github.com/yukihamada/nanobot)

### Manual Deploy

```bash
# Install Railway CLI
npm i -g @railway/cli

# Login
railway login

# Initialize project
railway init

# Add variables
railway variables set OPENAI_API_KEY=sk-...

# Deploy
railway up
```

---

## Vercel (Serverless Functions)

**Note:** Vercel has a 50MB deployment limit. nanobot binary is 4.6MB, so it fits.

### vercel.json

```json
{
  "builds": [
    {
      "src": "target/release/nanobot",
      "use": "@vercel/static-build"
    }
  ],
  "routes": [
    {
      "src": "/(.*)",
      "dest": "/api/nanobot"
    }
  ]
}
```

---

## VPS (Ubuntu/Debian)

### Step 1: Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

### Step 2: Clone and Build

```bash
git clone https://github.com/yukihamada/nanobot.git
cd nanobot
cargo build --release
```

### Step 3: Create systemd Service

```bash
sudo tee /etc/systemd/system/nanobot.service > /dev/null <<EOF
[Unit]
Description=nanobot AI Agent
After=network.target

[Service]
Type=simple
User=$USER
WorkingDirectory=$HOME/nanobot
Environment="OPENAI_API_KEY=sk-..."
Environment="ANTHROPIC_API_KEY=sk-ant-..."
ExecStart=$HOME/nanobot/target/release/nanobot gateway --http --http-port 3000
Restart=always

[Install]
WantedBy=multi-user.target
EOF
```

### Step 4: Enable and Start

```bash
sudo systemctl daemon-reload
sudo systemctl enable nanobot
sudo systemctl start nanobot
sudo systemctl status nanobot
```

### Step 5: Reverse Proxy (Nginx)

```nginx
server {
    listen 80;
    server_name your-domain.com;

    location / {
        proxy_pass http://localhost:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
    }
}
```

```bash
sudo systemctl reload nginx
```

---

## Environment Variables

See [environment-variables.md](environment-variables.md) for a complete list.

---

## Troubleshooting

### Cold Start Too Slow
- Use Lambda ARM64 (not x86_64)
- Increase memory to 512MB
- Enable Lambda SnapStart (if available)

### Out of Memory
- Increase Lambda memory to 512MB or 1GB
- Check for memory leaks in custom tools

### DynamoDB Throttling
- Switch from provisioned to on-demand billing
- Enable auto-scaling if using provisioned

### CORS Errors
- Add your domain to `ALLOWED_ORIGINS`
- Check API Gateway CORS configuration

---

**Need help?** Open an issue on [GitHub](https://github.com/yukihamada/nanobot/issues) or check [chatweb.ai/docs](https://chatweb.ai/docs).
