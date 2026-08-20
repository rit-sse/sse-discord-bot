FROM rust:1.97.1-bookworm AS builder

WORKDIR /usr/src/sse-discord-bot

COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src

RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home sse-discord-bot

COPY --from=builder \
    /usr/src/sse-discord-bot/target/release/sse-discord-bot \
    /usr/local/bin/sse-discord-bot

USER 10001

ENTRYPOINT ["/usr/local/bin/sse-discord-bot"]
