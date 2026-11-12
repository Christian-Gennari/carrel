# Stylos dev container: everything needed to build and test Stylos.
FROM debian:trixie
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git build-essential pkg-config python3 python3-pip \
      libgtk-4-dev libgtksourceview-5-dev libdbus-1-dev libsqlite3-dev libssl-dev \
      mkosi systemd-container systemd-boot-efi systemd-ukify dosfstools mtools \
      erofs-utils squashfs-tools e2fsprogs cpio zstd debian-archive-keyring \
      qemu-system-x86 qemu-utils ovmf \
      cage weston xvfb imagemagick \
      pandoc cmake \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain stable -c rustfmt,clippy && chmod -R a+rwX /opt/rustup /opt/cargo
WORKDIR /work
