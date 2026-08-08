# 1. 公式の最新Rust環境を使ってビルドする
FROM rust:latest AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

# 2. 実行用の軽量なコンテナにビルド済みバイナリだけを移す
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/discordbot /usr/local/bin/discordbot

# 3. ボットの起動コマンド
CMD ["discordbot"]
