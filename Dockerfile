# ---- build ----
FROM rust:1-slim AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --bin tideline-server

# ---- runtime ----
FROM debian:bookworm-slim
RUN useradd -m app
COPY --from=build /app/target/release/tideline-server /usr/local/bin/tideline-server
# The record is the product, so the image ships a durable default. Mount a
# volume at /data — without one the file lives in the container's writable
# layer and dies with the container.
RUN mkdir -p /data && chown app:app /data
VOLUME ["/data"]
ENV TIDELINE_RECORD_DB=/data/tideline.db

USER app
EXPOSE 8080
ENV RUST_LOG=info
CMD ["tideline-server"]
