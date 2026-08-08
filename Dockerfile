
FROM rust:latest AS builder
WORKDIR /app
COPY . .
RUN cargo build --release


FROM debian:bookworm-slim
WORKDIR /app


RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*


COPY --from=builder /app/target/release/discord_deepseek_bot /app/discord_deepseek_bot


CMD ["./discord_deepseek_bot"]
