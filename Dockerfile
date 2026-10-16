FROM lukemathwalker/cargo-chef:latest AS chef
WORKDIR /app

FROM chef AS planner
COPY ./Cargo.toml ./Cargo.lock ./
COPY ./src ./src
RUN cargo chef prepare

FROM chef AS builder
COPY --from=planner /app/recipe.json .
RUN cargo chef cook --release

COPY . .
# Declared after the dependency build so a new version doesn't invalidate its cache
ARG APP_VERSION
RUN APP_VERSION=$APP_VERSION cargo build --release
RUN mv ./target/release/dogbot ./app

FROM debian:trixie-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends libpq5 ca-certificates \
 && rm -rf /var/lib/apt/lists/*

ARG APP_VERSION
LABEL org.opencontainers.image.version=$APP_VERSION

WORKDIR /app
COPY --from=builder /app/app /usr/local/bin/

ENTRYPOINT ["/usr/local/bin/app"]