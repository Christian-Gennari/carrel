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
PKG_CACHE  := $(ROOT)/mkosi.cache

.PHONY: image test-boot clean

image:
	mkdir -p "$(IMAGE_DIR)" "$(PKG_CACHE)"
	mkosi --directory "$(ROOT)/image" \
	      --output-dir "$(IMAGE_DIR)" \
	      --package-cache-directory "$(PKG_CACHE)" \
	      --force build

test-boot:
	@if [ ! -f "$(IMAGE)" ]; then \
		echo "image missing, building it first"; \
		$(MAKE) image; \
	fi
	python3 "$(ROOT)/tests/boot/run.py" --image "$(IMAGE)" --out "$(OUT)"

clean:
	rm -rf "$(OUT)" "$(PKG_CACHE)" "$(ROOT)/mkosi.workspace"
