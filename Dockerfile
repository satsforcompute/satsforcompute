FROM rust:1.86-slim AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssh-client \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/dd-market /usr/local/bin/dd-market
ENTRYPOINT ["dd-market"]
