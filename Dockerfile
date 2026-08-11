# ---- build ----
FROM rust:1-slim AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin tideline

# ---- runtime ----
FROM debian:bookworm-slim
RUN useradd -m app
COPY --from=build /app/target/release/tideline /usr/local/bin/tideline
USER app
EXPOSE 8080
ENV RUST_LOG=info
CMD ["tideline"]
