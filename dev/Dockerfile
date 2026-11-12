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
      cage weston xvfb imagemagick tesseract-ocr \
      pandoc poppler-utils cmake \
    && rm -rf /var/lib/apt/lists/*
# Debian trixie does not package the typst CLI, but PDF export needs it
# (spec/01-architecture.md). Keep version and checksum in sync with
# image/mkosi.prepare.
RUN version=0.15.1 \
    && sha256=a6d077d0a95eed5a2eba715b2dae06be954f624ccbf85758a03f389ded33118c \
    && url="https://github.com/typst/typst/releases/download/v${version}/typst-x86_64-unknown-linux-musl.tar.xz" \
    && tmp="$(mktemp -d)" \
    && curl -fsSL "$url" -o "$tmp/typst.tar.xz" \
    && echo "$sha256  $tmp/typst.tar.xz" | sha256sum -c - \
    && tar -xJf "$tmp/typst.tar.xz" -C "$tmp" \
    && install -D -m 0755 "$tmp/typst-x86_64-unknown-linux-musl/typst" /usr/local/bin/typst \
    && rm -rf "$tmp"
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
