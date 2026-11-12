# Stylos dev container: everything needed to build and test Stylos.
FROM debian:trixie
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git build-essential pkg-config python3 python3-pip \
      python3-pefile \
      libgtk-4-dev libgtksourceview-5-dev libdbus-1-dev libsqlite3-dev libssl-dev \
      mkosi systemd-container systemd-boot-efi systemd-ukify dosfstools mtools \
      grub-pc-bin grub-efi-amd64-bin \
      erofs-utils squashfs-tools e2fsprogs cpio zstd xz-utils file gdisk \
      debian-archive-keyring \
      qemu-system-x86 qemu-utils ovmf \
      cage weston xvfb imagemagick \
      pandoc cmake \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
      --default-toolchain stable -c rustfmt,clippy && chmod -R a+rwX /opt/rustup /opt/cargo
# Login shells reset PATH; keep rustup available for `bash -lc` too.
RUN echo 'export PATH=/opt/cargo/bin:$PATH' > /etc/profile.d/rust.sh
# The cargo caches are named volumes. Make sure a fresh volume (and a volume
# created by an older version of this image) is writable for the host user.
RUN mkdir -p /opt/cargo/registry /opt/cargo/git && chmod -R a+rwX /opt/cargo
COPY --chmod=0755 entrypoint.sh /usr/local/bin/stylos-dev-entrypoint
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/stylos-dev-entrypoint"]
