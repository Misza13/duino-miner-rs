# Build stage
FROM alpine:latest as build

RUN apk add --no-cache rust cargo pkgconf build-base openssl-dev

RUN USER=root cargo new --bin app
WORKDIR /app

COPY ./src ./src
COPY Cargo.toml Cargo.lock ./

RUN cargo build --release


# Runtime stage
FROM alpine:latest

RUN apk add --no-cache libgcc openssl

COPY --from=build /app/target/release/duino-miner /usr/local/bin/duino-miner

CMD ["duino-miner"]