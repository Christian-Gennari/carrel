# carrel-server

A self-hosted sync server for Carrel. One static binary (plus a small Docker
image) that implements [Carrel Sync Protocol v1](../spec/02-sync-protocol.md).

- Files are plain Markdown in a normal folder. The path in the protocol is the
  path on disk, so any computer can read them without Carrel.
- A SQLite index at `<data-dir>/.carrel/index.sqlite` stores file ids,
  revisions, hashes, tombstones and the change cursor.
- TLS uses a self-signed certificate generated on first run. Its SHA-256
  fingerprint is returned during pairing, and devices pin it.
- Pairing is approved on a minimal HTML page at `/approve`, protected by an
  admin password. No JavaScript, no accounts, no cookies.
- The server announces `_carrel._tcp` over mDNS with `v=1` and
  `name=<label>`.

## Run

```sh
cargo run -p carrel-server -- --data-dir /srv/carrel --listen 0.0.0.0:8443
```

On first run it prints the admin password and certificate fingerprint, and
stores them under `<data-dir>/.carrel/`. Alternatively set them explicitly:

| Flag | Environment | Default |
|---|---|---|
| `--data-dir` | `CARREL_DATA_DIR` | `/data` |
| `--listen` | `CARREL_LISTEN` | `0.0.0.0:8443` |
| `--admin-password` | `CARREL_ADMIN_PASSWORD` | generated, stored |
| `--label` | `CARREL_LABEL` | host name |
| `--no-mdns` | `CARREL_NO_MDNS` | announce |

Then open `https://<server>:8443/approve` from a phone or computer on the same
network. The device shows a code such as `K7Q-4MZ`; approve it there.

## Docker

Build from the repository root (the workspace needs `protocol/` and
`carreld/` too):

```sh
docker build -f server/Dockerfile -t carrel-server .
docker run -d --name carrel-server \
  -p 8443:8443 \
  -v carrel-data:/data \
  carrel-server
docker logs carrel-server   # first-run password and fingerprint
```

The container runs as uid 1000. If you bind-mount a host directory instead of a
named volume, make sure uid 1000 can write to it.

## Notes

- `/library` returns `404`, as the protocol requires for self-hosted servers;
  quotes need a Nostos account.
- The admin page uses HTTP Basic authentication plus a per-process CSRF token.
  Use a long password if the server is reachable beyond your LAN.
- Pairing codes expire after ten minutes. Bearer tokens do not expire; unpair
  support arrives with the daemon's D-Bus API in a later work package.
- Hidden path components (`.carrel`, dotfiles) are rejected so the sync
  protocol can never write into the server's own metadata directory.
