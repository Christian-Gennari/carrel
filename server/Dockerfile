# Build from the repository root:
#   docker build -f server/Dockerfile -t carrel-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p carrel-server

FROM debian:bookworm-slim
RUN useradd --system --uid 1000 --home-dir /data --shell /usr/sbin/nologin carrel \
    && mkdir -p /data \
    && chown carrel:carrel /data
COPY --from=build /src/target/release/carrel-server /usr/local/bin/carrel-server
USER carrel
VOLUME ["/data"]
EXPOSE 8443
ENV CARREL_DATA_DIR=/data \
    CARREL_LISTEN=0.0.0.0:8443
ENTRYPOINT ["carrel-server"]
