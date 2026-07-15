FROM rust:1.97.0-slim-bookworm AS builder

ARG MIHOYO_BBS_TOOLS_VERSION=container
ARG GIT_COMMIT=unknown

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY config ./config
COPY integrations ./integrations
COPY src ./src

RUN MIHOYO_BBS_TOOLS_VERSION="$MIHOYO_BBS_TOOLS_VERSION" \
    GIT_COMMIT="$GIT_COMMIT" \
    cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libgcc-s1 tzdata \
    && groupadd --gid 10001 app \
    && useradd --uid 10001 --gid app --home-dir /app --create-home --shell /usr/sbin/nologin app \
    && install -d -o app -g app /app/logs \
    && rm -rf /var/lib/apt/lists/*

ENV TZ=Asia/Shanghai
WORKDIR /app

COPY --from=builder /app/target/release/MihoyoBBSToolsRS /usr/local/bin/MihoyoBBSToolsRS

USER app

ENTRYPOINT ["MihoyoBBSToolsRS"]
CMD ["run"]
