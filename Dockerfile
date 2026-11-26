FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY public ./public
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin wzq
ENV WZQ_HISTORY_DIR=/var/lib/wzq/history
RUN install -d -o wzq -g "$(id -gn wzq)" /var/lib/wzq/history
VOLUME ["/var/lib/wzq/history"]
COPY --from=build /app/target/release/wzq-server /usr/local/bin/wzq-server
ENV HOST=0.0.0.0 PORT=3000
EXPOSE 3000
USER 10001
CMD ["wzq-server"]
