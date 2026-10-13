# Staging validation (gated live procedure)

**Status:** Procedure only. No live validation has been claimed: every live
input below is absent, so every live gate in this file is PENDING. Local
proofs (disposable stacks, synthetic provider responses) are recorded
separately and never stand in for this procedure.

Companion status: `docs/engineering/release-status.md`. Backup detail:
`backups.md`. Deploy detail: `deployment.md`.

## Required inputs (all must exist before any live step)

- Approved staging host, access method, and authorized operator.
- Domain/DNS name (or approved test hostname) with TLS configuration.
- Runtime secrets installed on the host (never in the repository).
- Live phone-verification service: authorized account, credentials, budget
  cap, permitted destination scope, and consent/notice configuration. The
  planned adapter is Twilio Verify; no provider is purchased or activated
  by implementation work.
- One test destination number owned by the project for bounded proofs.
- Approved off-host backup destination with encryption/key custody,
  retention, restore credentials, and schedule.

If any input is missing, stop: record which one, keep prior local results
valid, and claim nothing about the corresponding live gate. A synthetic
fake success never proves real phone control, and a local restore never
proves the off-host destination.

## Procedure A — bounded live phone control

1. Confirm the budget cap and destination scope with the owner; send exactly
   one challenge to the test destination only. Bulk or production numbers
   are out of bounds — abort on any such instruction.
2. Confirm the code through the real provider path and assert the account
   activates; then assert a wrong code still fails. Record the provider
   outcome (approved/denied), never the code.
3. Assert production startup still refuses the deterministic fake provider.

## Procedure B — real HTTPS cookie/session/flow smoke

1. Against the authorized origin only: `GET /health/live` (alive),
   `GET /health/ready` (ready), one public request read (no phone-shaped
   material), and one cookie-less protected read (refused, no message sent).
   `scripts/smoke.sh --database-url ... --base-url ...` encodes these probes
   (GET only); point it at the staging origin when the serving binary lands.
2. Assert cookie transport: `HttpOnly`, `Secure`, and explicit
   `SameSite`/origin handling on session cookies over HTTPS.
3. Exercise one registration-to-verification round trip with the test
   destination; assert session authentication follows current server state.

## Procedure C — restore from the actual destination

1. Take a backup per `backups.md` and ship it to the approved destination.
2. Restore into an isolated target from that destination (not from a local
   copy) and rerun the core flow invariants. Record artifact checksums,
   host, and result.

## Evidence to record

Tested artifact SHA, host, provider outcome, restored counts, and each
gate's pass/pending state with the missing input named where pending.
Pending gates never become passes without a new run after the input lands.
