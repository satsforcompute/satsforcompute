# Build a static (musl) `satsforcompute` binary in a builder stage,
# then copy it into a minimal scratch image. Same pattern as
# devopsdefender/dd's release.yml so the operator workload can ride
# the same EE github_release fetch path.

FROM rust:1.86-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/satsforcompute /satsforcompute
ENTRYPOINT ["/satsforcompute"]
