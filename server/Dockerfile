# Build from the repository root:
#   docker build -f server/Dockerfile -t mint-server .
# (or `docker compose up -d --build` in server/)

FROM rust:1-slim-bookworm AS build
# The workspace uses cargo's nightly-only artifact dependencies (for the hook DLL), so even the
# server needs a nightly cargo. RUSTUP_TOOLCHAIN overrides rust-toolchain.toml, which would also
# pull in the Windows target.
ENV RUSTUP_TOOLCHAIN=nightly
RUN rustup toolchain install nightly --profile minimal
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p mint_server \
    && cp target/release/mint_server /usr/local/bin/mint_server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home mint \
    && mkdir /data && chown mint /data
COPY --from=build /usr/local/bin/mint_server /usr/local/bin/mint_server
USER mint
ENV MINT_SERVER_DATA=/data MINT_SERVER_LISTEN=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
CMD ["mint_server"]
