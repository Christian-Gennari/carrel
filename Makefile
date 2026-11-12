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

# Release keys never live in git.  `make keys` creates the development pair in
# this directory and copies only the public certificate/keyring to image/keys.
STYLOS_KEY_DIR ?= $(HOME)/.config/stylos/keys
VERITY_KEY     ?= $(STYLOS_KEY_DIR)/verity.key
VERITY_CERT    ?= $(STYLOS_KEY_DIR)/verity.crt
UPDATE_SECRET  ?= $(STYLOS_KEY_DIR)/update-secret.gpg
UPDATE_PUBKEY  ?= $(ROOT)/image/keys/update-pubring.gpg
RELEASE_VERSION ?= 0.2.0

# Where an installed device looks for signed update payloads.  It contains
# SHA256SUMS and SHA256SUMS.gpg at the root.  Override for a private mirror.
STYLOS_UPDATE_URL ?= https://updates.stylos.example/stylos/

.PHONY: image binaries test-boot keys release clean

# Generate the development release key pair outside the repository and copy
# the public certificate/keyring into image/keys/.  Set STYLOS_FORCE_KEYS=1 to
# replace existing keys (then rebuild and commit the new public files).
keys:
	python3 "$(ROOT)/image/make-keys"

# Build the real shells in the dev container (Rust + the cargo cache are here)
# and let the mkosi build step install them.  Never committed: target/ is
# ignored.  mkosi mounts the repo at $SRCDIR, so the binaries are visible to
# mkosi.build at $SRCDIR/target/release.
binaries:
	CARGO_TARGET_DIR="$(ROOT)/target" cargo build --release -p stylos-shell -p stylosd

image: binaries
	@test -f "$(VERITY_KEY)" || { \
		echo "missing $(VERITY_KEY); run 'make keys' first"; exit 1; }
	@test -f "$(VERITY_CERT)" || { \
		echo "missing $(VERITY_CERT); run 'make keys' first"; exit 1; }
	@test -f "$(UPDATE_PUBKEY)" || { \
		echo "missing $(UPDATE_PUBKEY); run 'make keys' first"; exit 1; }
	@set -e; \
	trap 'rm -rf "$(ROOT)/.stylos-keys"' EXIT; \
	mkdir -p "$(IMAGE_DIR)" "$(PKG_CACHE)" "$(ROOT)/.stylos-keys"; \
	install -m 0600 "$(VERITY_KEY)" "$(ROOT)/.stylos-keys/verity.key"; \
	install -m 0644 "$(VERITY_CERT)" "$(ROOT)/.stylos-keys/verity.crt"; \
	STYLOS_UPDATE_URL="$(STYLOS_UPDATE_URL)" \
	mkosi --directory "$(ROOT)/image" \
	      --build-sources "$(ROOT)" \
	      --output-dir "$(IMAGE_DIR)" \
	      --package-cache-directory "$(PKG_CACHE)" \
	      --verity-key="$(ROOT)/.stylos-keys/verity.key" \
	      --verity-certificate="$(ROOT)/.stylos-keys/verity.crt" \
	      --environment STYLOS_VERITY_KEY=.stylos-keys/verity.key \
	      --environment STYLOS_VERITY_CERT=.stylos-keys/verity.crt \
	      --environment STYLOS_UPDATE_URL="$(STYLOS_UPDATE_URL)" \
	      $(if $(STYLOS_OAUTH_TOML),--environment STYLOS_OAUTH_TOML=$(STYLOS_OAUTH_TOML)) \
	      --force build

test-boot:
	@if [ ! -f "$(IMAGE)" ]; then \
		echo "image missing, building it first"; \
		$(MAKE) image; \
	fi
	@if [ ! -f "$(OUT)/release/SHA256SUMS.gpg" ]; then \
		echo "signed release missing, building it first"; \
		$(MAKE) release; \
	fi
	python3 "$(ROOT)/tests/boot/run.py" --image "$(IMAGE)" \
		--update-dir "$(IMAGE_DIR)/update" --release-dir "$(OUT)/release" \
		--out "$(OUT)"

# Build signed online-update payloads from the image just built.  The image
# must have been built first; the private signing key stays outside git.
release:
	python3 "$(ROOT)/image/release.py" --image "$(IMAGE)" \
		--out "$(OUT)/release" --version "$(RELEASE_VERSION)" \
		--verity-key "$(VERITY_KEY)" --verity-cert "$(VERITY_CERT)" \
		--update-secret "$(UPDATE_SECRET)"

clean:
	rm -rf "$(OUT)" "$(PKG_CACHE)" "$(ROOT)/mkosi.workspace"
