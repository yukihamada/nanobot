FROM rust:1.83-slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release --features http-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/chatweb /usr/local/bin/chatweb
COPY --from=builder /app/web /web
ENV NANOBOT_WEB_DIR=/web
EXPOSE 3000
CMD ["chatweb", "gateway", "--http", "--http-port", "3000"]
