FROM rust:latest

WORKDIR /app
COPY . .

# 依存関係を含めてビルドし、そのままバイナリを実行する
RUN cargo build --release

CMD ["./target/release/discordbot"]
