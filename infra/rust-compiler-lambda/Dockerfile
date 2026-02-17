# Lambda上でRustをコンパイル＆実行するDockerイメージ
FROM public.ecr.aws/lambda/provided:al2023

# Rustのインストール
RUN yum install -y gcc && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable && \
    . $HOME/.cargo/env && \
    rustup target add x86_64-unknown-linux-musl

# 環境変数
ENV PATH="/root/.cargo/bin:${PATH}"
ENV RUSTFLAGS="-C target-feature=+crt-static"

# Lambda Runtime APIのハンドラー
COPY bootstrap.sh /bootstrap
RUN chmod +x /bootstrap

CMD ["/bootstrap"]
