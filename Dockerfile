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
USER app
EXPOSE 8080
ENV RUST_LOG=info
CMD ["tideline-server"]
