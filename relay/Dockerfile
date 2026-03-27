FROM rust:1.80-slim as builder
WORKDIR /usr/src/app
RUN apt-get update && apt-get install -y protobuf-compiler pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release --bin relay

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/app/target/release/relay /usr/local/bin/relay
EXPOSE 8000
VOLUME ["/data"]
WORKDIR /data
ENTRYPOINT ["relay"]
