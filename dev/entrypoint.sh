#!/bin/sh
# Entrypoint for the Stylos dev container.
#
# mkosi and the QEMU boot harness have to run as root. To keep the repository
# free of root-owned build outputs, this wrapper runs the requested command and
# then gives the generated files back to the host user when STYLOS_CHOWN=1.
set -u

status=0
if [ "$#" -gt 0 ]; then
    "$@" || status=$?
fi

if [ "${STYLOS_CHOWN:-0}" = "1" ] && [ "$(id -u)" = "0" ]; then
    for dir in \
        /work/out \
        /work/target \
        /work/mkosi.cache \
        /work/mkosi.workspace \
        /work/image/mkosi.cache \
        /work/image/mkosi.workspace \
        /opt/cargo/registry \
        /opt/cargo/git
    do
        if [ -e "$dir" ]; then
            chown -R "${STYLOS_UID:-0}:${STYLOS_GID:-0}" "$dir" 2>/dev/null || true
        fi
    done
fi

exit "$status"
