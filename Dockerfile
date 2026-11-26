FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY public ./public
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/wzq-server /usr/local/bin/wzq-server
ENV HOST=0.0.0.0 PORT=3000
EXPOSE 3000
CMD ["wzq-server"]
