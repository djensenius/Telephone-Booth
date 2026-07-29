# Operator API

The Rust client speaks a versioned `/v1` REST + WebSocket API to the
operator backend. The full schema is in
[`packages/api/openapi.yaml`](https://github.com/djensenius/Telephone-Booth-Operator/blob/main/packages/api/openapi.yaml)
in the operator repo.

## Authenticating

The phone client uses a **static Bearer API token** issued from the
operator UI:

```http
Authorization: Bearer tbo_4b…d8
```

Tokens are 32 random bytes encoded as URL-safe base64, prefixed `tbo_`.

### Issuing a token

1. Sign into the operator UI (Authentik OIDC).
2. Dial **6** on the rotary nav → **Settings**.
3. **API tokens → Create** → give it a label like `booth-1`.
4. **Copy** the plaintext token — it's shown only once.
5. Paste it into `/etc/phone-booth/config.toml`:

   ```toml
   [operator]
   token = "tbo_…"
   ```

6. `sudo systemctl restart telephone-booth`.

### Rotating a token

1. Operator UI → Settings → API tokens → **Create** a new token.
2. Drop it into the Pi's config and restart the service.
3. Operator UI → **Revoke** the old token.

Rotation is intentionally a two-step pattern so a botched paste doesn't
take the booth offline.

### Revoking

`DELETE /v1/api-tokens/{id}` in the operator UI immediately stops
accepting that token. The phone client will start logging `401`s; the
debug panel will show "Operator: unauthenticated".

## What the client calls

| Verb / path                          | Purpose                                                                         |
| ------------------------------------ | ------------------------------------------------------------------------------- |
| `PUT  /v1/status`                    | Posts the current `BoothStatus` whenever it changes                              |
| `GET  /v1/questions/random`          | After dialing **1**, fetch a random approved question to play                    |
| `GET  /v1/messages/random`           | After dialing **2**, fetch a random approved message to play                     |
| `POST /v1/messages`                  | Create a message row and request a presigned Azure Blob upload URL              |
| `PUT  <SAS URL>`                     | Upload the FLAC directly to Azure Blob Storage (requires `x-ms-blob-type: BlockBlob` and `x-ms-meta-sha256: <hex>`) |
| `POST /v1/messages/{id}/complete`    | Ask the API to verify the uploaded blob and mark the message received           |
| `WS   /v1/ws/status`                 | _(reverse direction)_ Operator UI subscribes to status; the booth pushes events  |

The WebSocket is **operator-side only** — the phone client doesn't open
it. Status updates from the phone client are HTTP `PUT`s; the operator
backend fan-outs to connected browsers.

## Audit trail

Every write the client makes is recorded by the operator backend with the
acting principal, the client IP, the request path, the response status and a
timestamp. See
[`docs/audit-log.md`](https://github.com/djensenius/Telephone-Booth-Operator/blob/main/docs/audit-log.md)
in the operator repo.

There is no operator user behind these calls, so **the API token _is_ the
identity**. The trail shows `token:<label>`, which is the label typed when the
token was created — so give each booth its own token with a label that names
the machine (`booth-1`, `booth-lobby`), never a shared one. A revoked token
keeps its history: entries are append-only and the label was captured at the
time of the action.

Which calls are recorded:

| Call                              | Recorded as         | Notes                                    |
| --------------------------------- | ------------------- | ---------------------------------------- |
| `POST /v1/messages`               | `message.create`    | Includes a failed attempt (`401`/`409`)  |
| `POST /v1/messages/{id}/complete` | `message.complete`  | Records the verification outcome         |
| `PUT  /v1/status`                 | —                   | Telemetry; excluded by default           |
| `PUT  /v1/system`                 | —                   | Telemetry; excluded by default           |
| `POST /v1/events`                 | —                   | Telemetry; excluded by default           |
| `GET` calls                       | —                   | Reads are never audited                  |

Status, system and event pushes are heartbeats that would otherwise bury the
trail in noise, so the operator backend skips them unless an admin sets
`AUDIT_LOG_TELEMETRY=true`. They are still visible in the sessions and events
screens, which is where they belong.

Rejected writes are recorded too — that is the point. If a booth's token is
revoked mid-shift, the `401`s show up in the trail with the token label, the IP
and the time, which is usually the fastest way to confirm what happened.

The client sends `User-Agent: telephone-booth/<version>`, which the backend
stores alongside each entry, so it is easy to tell a booth's writes apart from
a `curl` run by hand with the same token.

## Common errors

| HTTP   | Likely cause                                                       |
| ------ | ------------------------------------------------------------------ |
| `401`  | API token wrong or revoked. Reissue from the operator UI.          |
| `403`  | Token valid but lacks scope (shouldn't happen with current schema).|
| `409`  | Message `sha256` already exists or the completion blob is missing. On reboot, a recording whose message row was created but whose blob never uploaded needs the operator's idempotent re-initiation (returns a fresh SAS for `uploading` messages) to recover. |
| `413`  | Uploaded audio exceeds the 25 MiB operator cap.                     |
| `422`  | Blob verification failed, usually missing/mismatched SHA metadata.  |
| `400`/`422` | Azure `PUT <SAS URL>` upload omitted `x-ms-blob-type: BlockBlob` (`MissingRequiredHeader`) or `x-ms-meta-sha256` (`/complete` returns `sha256_metadata_missing`). The phone client sends both; a bare `curl` won't. |
| `5xx`  | Operator backend down. The client retries with exponential backoff. |
