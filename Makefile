# Stylos top-level build targets.
#
# Everything runs inside the dev container:
#   dev/run.sh --root make image
#   dev/run.sh --root make test-boot
#
# `make test-boot` reuses an existing image; run `make image` first (the gate
# does exactly that).

SHELL := /bin/sh

ROOT       := $(CURDIR)
OUT        := $(ROOT)/out
IMAGE_DIR  := $(OUT)/image
IMAGE      := $(IMAGE_DIR)/stylos.img.raw
BINARIES   := $(ROOT)/target/release
PKG_CACHE  := $(ROOT)/mkosi.cache

.PHONY: image binaries test-boot clean

# Build the real shells in the dev container (Rust + the cargo cache are here)
# and let the mkosi build step install them.  Never committed: target/ is
# ignored.  mkosi mounts the repo at $SRCDIR, so the binaries are visible to
# mkosi.build at $SRCDIR/target/release.
binaries:
	CARGO_TARGET_DIR="$(ROOT)/target" cargo build --release -p stylos-shell -p stylosd

image: binaries
	mkdir -p "$(IMAGE_DIR)" "$(PKG_CACHE)"
	mkosi --directory "$(ROOT)/image" \
	      --build-sources "$(ROOT)" \
	      --output-dir "$(IMAGE_DIR)" \
	      --package-cache-directory "$(PKG_CACHE)" \
	      $(if $(STYLOS_OAUTH_TOML),--environment STYLOS_OAUTH_TOML=$(STYLOS_OAUTH_TOML)) \
	      --force build

test-boot:
	@if [ ! -f "$(IMAGE)" ]; then \
		echo "image missing, building it first"; \
		$(MAKE) image; \
	fi
	python3 "$(ROOT)/tests/boot/run.py" --image "$(IMAGE)" \
		--update-dir "$(IMAGE_DIR)/update" --out "$(OUT)"

clean:
	rm -rf "$(OUT)" "$(PKG_CACHE)" "$(ROOT)/mkosi.workspace"
