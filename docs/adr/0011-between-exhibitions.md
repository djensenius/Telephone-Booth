# ADR 0011 - Between-exhibitions admission and durable replay

**Status:** accepted.

## Context

Ending an exhibition is an operator decision, not a network outage. A powered-on
booth must stop offering calls until an operator explicitly starts again, while
preserving answers already being recorded and recordings awaiting upload.
Restarting the process must not be necessary to resume pending work.

## Decision

- Reconcile `GET /v1/status` with the existing booth bearer token on a separate,
  single task every five seconds, with a five-second deadline and no probe retries.
  Never await the network in the core, GPIO loop, or watchdog loop.
- Treat `installationState: between_exhibitions` as authoritative and sticky
  across failed probes. Only a subsequent successful status response can reopen
  admission. Missing `installationState` preserves legacy behavior; it does not
  mean inactive. Ignore status timestamps and `isSynthetic` for admission:
  an epoch-dated synthetic row can carry lifecycle state without being a heartbeat.
- Start with admission closed until the first response. An explicit `active`
  response leases admission for 15 seconds; if it becomes stale, pause calls as
  *unknown*, not as confirmed between-exhibitions. Continue reporting actual
  authentication, protocol, and transport failures through logs and telemetry.
- Centralize exhibition-operation gating in an `OperatorClient` decorator.
  Status writes, event batches, random content, upload initiation, blob transfer,
  and completion are deferred while closed; system snapshots remain available.
  An exact HTTP 409 JSON `error: installation_inactive` closes admission
  immediately and invalidates any older in-flight status response.
- Keep call admission pure via `handle_with_call_availability`. `CallsPaused`
  remembers hook position, plays nothing, and is not a call or error session.
  Seed hook position from the physical GPIO snapshot at boot; a handset already
  lifted resumes correctly without requiring a new edge.
  Apply the first dial input even when it arrives before the resume notification.
  An interrupted existing call reports `aborted`, not a fabricated caller hangup;
  confirmed upload outcomes are preserved.
  Preserve digit mappings. Cancel abandoned prompt results with the existing
  generation helper; never resolve cached prompts while paused.
- Do not interrupt recording or finalization. Sync the finalized FLAC and its
  directory, then persist the answer in the existing upload spool before network
  I/O. Sync spool metadata and its directory on the blocking pool, within the
  bounded upload tasks rather than the critical effect dispatcher. Fail
  explicitly if durable storage cannot open or enqueue.
  A deferral is not an upload acknowledgement or failure.
- Track durability jobs separately from cancellable network tasks. On shutdown,
  drain accepted recording-upload effects and their durable writes before
  closing the dispatcher, without waiting for slow operator requests.
- Replay recordings serially on admission reopening and every 30 seconds while
  open. Share per-recording claims with live uploads; retain failed entries,
  rotate past failures on later sweeps, and back off after a failed attempt.
  Recovered uploads never produce call-completion events. Live completion events
  carry recording identity so they cannot advance an unrelated caller's state.
- Persist even small event batches immediately on the new conflict, syncing
  the file and directory before clearing the buffer. Retain the buffer if
  persistence fails. Read and replay only the oldest spooled batch each forwarder
  flush tick, with filesystem work on the blocking pool. Keep existing
  event-spool retention limits. Persist batch sequence numbers in filenames so
  replay and retention stay ordered across restarts; legacy files precede new
  batches and use filesystem modification times for their ordering.
  Nothing on the booth starts an installation implicitly.

## Consequences

Manual end/start is noticed within a normal five-second poll cycle. An operation
already in flight can race the end; the server's typed conflict closes the gap
without deleting the recording. In-flight recording continues to normal hangup
or its configured limit. Polling and replay concurrency stay bounded.

Known-active booths temporarily stop offering new calls after a prolonged outage.
Legacy servers retain their previous behavior. Pending recordings need writable,
persistent disk space; storage failures remain operational errors, not pretend
success. The operator contract and the booth must both be deployed to enforce
the lifecycle across all clients.
