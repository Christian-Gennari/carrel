# Build from the repository root:
#   docker build -f server/Dockerfile -t stylos-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p stylos-server

FROM debian:bookworm-slim
RUN useradd --system --uid 1000 --home-dir /data --shell /usr/sbin/nologin stylos \
    && mkdir -p /data \
    && chown stylos:stylos /data
COPY --from=build /src/target/release/stylos-server /usr/local/bin/stylos-server
USER stylos
VOLUME ["/data"]
EXPOSE 8443
ENV STYLOS_DATA_DIR=/data \
    STYLOS_LISTEN=0.0.0.0:8443
ENTRYPOINT ["stylos-server"]
