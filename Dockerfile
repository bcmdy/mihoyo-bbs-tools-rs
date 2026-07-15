FROM rust:1-alpine AS builder

ARG MIHOYO_BBS_TOOLS_VERSION=container

RUN apk add --no-cache musl-dev

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY config ./config
COPY integrations ./integrations
COPY src ./src

RUN MIHOYO_BBS_TOOLS_VERSION="$MIHOYO_BBS_TOOLS_VERSION" cargo build --release --locked

FROM alpine:3

RUN apk add --no-cache ca-certificates tzdata \
    && addgroup -S -g 10001 app \
    && adduser -S -D -H -u 10001 -G app app

ENV TZ=Asia/Shanghai
WORKDIR /app

COPY --from=builder /app/target/release/MihoyoBBSToolsRS /usr/local/bin/MihoyoBBSToolsRS

USER app

ENTRYPOINT ["MihoyoBBSToolsRS"]
CMD ["run"]
