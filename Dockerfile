# ビルドステージ
FROM rust:latest AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

# 実行ステージ
FROM debian:bookworm-slim
WORKDIR /app

# 必要なSSL証明書や依存関係をインストール
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# ビルド成果物をコピー
COPY --from=builder /app/target/release/discord_deepseek_bot /app/discord_deepseek_bot

# 実行
CMD ["./discord_deepseek_bot"]
