#!/bin/sh
# Entrypoint for the Carrel dev container.
#
# mkosi and the QEMU boot harness have to run as root. To keep the repository
# free of root-owned build outputs, this wrapper runs the requested command and
# then gives the generated files back to the host user when CARREL_CHOWN=1.
# It does so also when the container is stopped (SIGTERM/SIGINT): the signal
# is forwarded to the command and the files are handed back before exit.
set -u

handback() {
    if [ "${CARREL_CHOWN:-0}" = "1" ] && [ "$(id -u)" = "0" ]; then
        /work/dev/chown-outputs /work
        for dir in /opt/cargo/registry /opt/cargo/git; do
            if [ -e "$dir" ]; then
                chown -R "${CARREL_UID:-0}:${CARREL_GID:-0}" "$dir" 2>/dev/null || true
            fi
        done
    fi
}

status=0
if [ "$#" -gt 0 ]; then
    if [ -t 0 ]; then
        # Interactive: the command needs the terminal in the foreground.
        "$@" || status=$?
    else
        child=
        forward() { [ -n "$child" ] && kill -TERM "$child" 2>/dev/null; }
        trap forward TERM INT
        exec 3<&0   # a background command would otherwise get /dev/null
        "$@" <&3 &
        child=$!
        wait "$child"
        status=$?
        # A trapped signal ends the first wait early; wait for the real exit.
        while kill -0 "$child" 2>/dev/null; do
            wait "$child"
            status=$?
        done
        trap - TERM INT
    fi
fi

handback
exit "$status"
