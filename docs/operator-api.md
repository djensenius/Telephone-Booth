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
| `PUT  <SAS URL>`                     | Upload the FLAC directly to Azure Blob Storage (requires `x-ms-blob-type: BlockBlob`) |
| `POST /v1/messages/{id}/complete`    | Ask the API to verify the uploaded blob and mark the message received           |
| `WS   /v1/ws/status`                 | _(reverse direction)_ Operator UI subscribes to status; the booth pushes events  |

The WebSocket is **operator-side only** — the phone client doesn't open
it. Status updates from the phone client are HTTP `PUT`s; the operator
backend fan-outs to connected browsers.

## Common errors

| HTTP   | Likely cause                                                       |
| ------ | ------------------------------------------------------------------ |
| `401`  | API token wrong or revoked. Reissue from the operator UI.          |
| `403`  | Token valid but lacks scope (shouldn't happen with current schema).|
| `409`  | Message `sha256` already exists or the completion blob is missing.  |
| `413`  | Uploaded audio exceeds the 25 MiB operator cap.                     |
| `422`  | Blob verification failed, usually missing/mismatched SHA metadata.  |
| `MissingRequiredHeader` (Azure XML) | The `PUT <SAS URL>` upload omitted `x-ms-blob-type: BlockBlob`. The phone client sends it; a bare `curl` won't. |
| `5xx`  | Operator backend down. The client retries with exponential backoff. |
