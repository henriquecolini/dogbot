FROM lukemathwalker/cargo-chef:latest AS chef
WORKDIR /app

FROM chef AS planner
COPY ./Cargo.toml ./Cargo.lock ./
COPY ./src ./src
RUN cargo chef prepare

FROM chef AS builder

ARG APP_VERSION
ENV APP_VERSION=$APP_VERSION

COPY --from=planner /app/recipe.json .
RUN cargo chef cook --release

COPY . .
RUN APP_VERSION=$APP_VERSION cargo build --release
RUN mv ./target/release/dogbot ./app

FROM debian:trixie-slim AS runtime

ARG APP_VERSION
ENV APP_VERSION=$APP_VERSION
LABEL org.opencontainers.image.version=$APP_VERSION

RUN apt-get update \
 && apt-get install -y --no-install-recommends libpq5 ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/app /usr/local/bin/

ENTRYPOINT ["/usr/local/bin/app"]