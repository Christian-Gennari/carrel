# Stylos Sync Protocol v1

HTTPS + JSON. Implemented by `stylos-server` and by Nostos Cloud. Types live
in the `stylos-protocol` crate (MIT). A device is a client; it only ever talks
when the user starts a sync or pairing.

## Discovery

- Nostos: base URL `https://api.nostos.app/stylos/v1` (configurable).
- Self-hosted on LAN: mDNS service `_stylos._tcp`, TXT `v=1`, `name=<label>`.
- Self-hosted via Tailscale or manual: the user enters a host name or URL.

## Pairing

1. `POST /pair/start` `{ "device_name": "X250", "device_pubkey": "<ed25519>" }`
   → `{ "pair_id", "code": "K7Q-4MZ", "expires_at" }`.
2. The device shows the code (and a QR with the approval URL).
3. The user approves the code: on Nostos at `nostos.app/pair`, on
   `stylos-server` at its web page (`/approve`).
4. The device polls `GET /pair/{pair_id}` (every 2 s, max 10 min)
   → `{ "status": "pending" | "approved" | "expired", "token", "server_cert_sha256" }`.
5. The device stores the token in `/data/.stylos/sync.toml` (mode 0600). For
   self-hosted servers it pins `server_cert_sha256` (self-signed is fine).

All later requests: `Authorization: Bearer <token>`.

## Files

A file is identified by a stable `file_id` (UUIDv7, created on the device or
server) and has a `path` (relative, `/`-separated, `.md`). Content hash =
`sha256` of the UTF-8 bytes.

- `GET /files?since=<cursor>` → `{ "files": [ { file_id, path, hash, rev,
  deleted, modified_at } ], "cursor" }`. The cursor is opaque.
- `GET /files/{file_id}` → `{ file_id, path, hash, rev, content }`.
- `PUT /files/{file_id}` `{ path, content, base_rev }`
  - `base_rev` matches the server's current rev (or `null` for a new file)
    → `200 { rev, hash }`.
  - Otherwise → `409 { "conflict": { server_rev, server_hash } }`. The server
    changes nothing.
- `DELETE /files/{file_id}?base_rev=…` → `200` or `409` (same rule).

## Sync algorithm (client)

1. `stylosd` commits local history.
2. Pull: `GET /files?since=cursor`. For each remote change:
   - Local file unchanged since last sync → apply remote.
   - Local file also changed → conflict.
3. Push: for each local file changed since last sync → `PUT` with `base_rev`.
   On `409` → conflict.
4. Conflict handling: never merge. Keep the local file. Write the other
   version next to it as `name (conflict YYYY-MM-DD HHMM from <source>).md`,
   then push both. The Files screen marks conflict copies; the user deletes the
   one they do not want.
5. Save the new cursor and per-file `rev` in `/data/.stylos/sync-state.sqlite`.

## Library (Nostos only)

- `GET /library?since=<cursor>` → `{ "books": [...], "items": [ { id, kind:
  "highlight"|"note"|"passage", book_id, text, note, location, topics,
  citation: { markdown, url } } ], "deleted": [ids], "cursor" }`.
- `stylos-server` returns `404` for `/library`; the device then shows "Quotes
  need a Nostos account".

## Cloud folder backends (Google Drive, Dropbox)

For users without Nostos or a server. Simplest safe model: a **one-way
mirror**. The device is the source of truth; each sync uploads changed files
to a `Stylos` folder in the user's cloud drive. Edits made in the cloud folder
do not come back (the Sync screen says so). Deleted files move to
`Stylos/.deleted/` in the cloud, never hard-deleted.

- Options per backend: mirror Markdown (default), and optionally also a
  `.docx` copy of each file (made with the export pipeline), so the files open
  directly in Google Docs / Word.
- Implemented natively in `stylosd` (Rust, reqwest). No rclone, no browser on
  the device.
- **Google Drive**: OAuth 2.0 **device authorization flow** ("TV and limited
  input devices" client). Scope `drive.file` only (the app sees only files it
  created; non-sensitive scope, no Google verification needed). The device
  shows a URL + short code + QR; the user approves on a phone. Upload with the
  Drive v3 API; keep a `path → file id` map in `sync-state.sqlite`.
- **Dropbox**: "App folder" app (sees only `Apps/Stylos/`). OAuth 2.0 PKCE with
  no redirect: the device shows a URL + QR, the user approves on a phone,
  Dropbox shows a code, the user types it on the device. Refresh token
  (`token_access_type=offline`). Upload with `/2/files/upload`; compare
  `content_hash` to skip unchanged files.
- OAuth client IDs come from `/usr/share/stylos/oauth.toml` (built into the
  image; overridable in `/data/.stylos/oauth.toml` for self-builders).
- More than one backend may be active (for example Nostos + Dropbox). Sync runs
  them in order in the same network session.

## Updates

Not part of this protocol. sysupdate fetches signed payloads from the Stylos
release URL during a sync session only.

## Error rules

- Any network error ends the sync, blocks the radio again and shows one line
  on the Sync screen. No retries in the background.
- Partial syncs are safe: every step is idempotent.
