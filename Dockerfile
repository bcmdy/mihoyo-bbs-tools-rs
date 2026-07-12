FROM rust:1-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

COPY Cargo.toml ./
COPY src ./src

RUN cargo generate-lockfile \
    && cargo build --release --locked

FROM alpine:3

RUN apk add --no-cache ca-certificates tzdata \
    && addgroup -S app \
    && adduser -S -G app app

ENV TZ=Asia/Shanghai
WORKDIR /app

COPY --from=builder /app/target/release/mihoyo-bbs-tools /usr/local/bin/mihoyo-bbs-tools

USER app

ENTRYPOINT ["mihoyo-bbs-tools"]
CMD ["run"]
