# Follow-up plan — worker↔server consolidation & dual-server admin

These items came out of the consolidation audit but were **excluded from the
`/simplify` cleanup pass** because each either touches a deployed hot path
(signing / upload / pull) that can only be wasm-compile-verified here, not
integration-tested, or requires **new features** rather than cleanups. This is
the pickup list, ordered by value ÷ risk.

## Prerequisite — worker integration harness

Several workstreams below are only safe with an end-to-end test. Build this
first: push → pull → narinfo signature-verify against a throwaway cache
(miniflare + local R2, or a scratch cache on the live worker), covering the
matrix: {buffered small, streaming >15 MB, chunked, dedup} × {none, zstd, br,
xz, gzip}. Assert stored objects are byte-identical and `nix` accepts the
signature. Without this, Workstreams A3 and C are unsafe to land.

---

## Workstream A — Worker ↔ attic-core consolidation (shared Rust)

Goal: the worker reuses `attic` core instead of re-implementing crypto/hash/wire
types. Highest reuse payoff; do the low-blast-radius parts first.

- **A1. Extract `compute_fingerprint` into `attic` core.** Add a `std`-only free
  fn `compute_fingerprint(store_path, nar_hash, nar_size, refs) -> Vec<u8>` in
  `attic/src/signing`. Have `worker/src/crypto.rs` and
  `server/src/narinfo` (`NarInfo::fingerprint`) delegate. *Verify:* unit test
  against known vectors; wasm compile. *Risk:* low.
- **A2. Replace `convert_hash_to_base32`** (`worker/src/crypto.rs`) with
  `attic::hash::Hash::{from_typed, to_typed_base32}`. *Care:* preserve the
  current "return input unchanged on parse failure" behavior, or prove inputs
  are always valid. Add a test at the two `binary_cache.rs` call sites. *Risk:* low.
- **A3. Replace worker signing with `attic::signing::NixKeypair`.** Delete the
  `generate_keypair`/`extract_public_key`/`sign_message`/`decode_keypair`
  functions and the duplicate `ed25519-dalek` dependency (`attic` uses
  wasm-friendly `ed25519-compact`). *Verify:* **signature parity** — sign a known
  fingerprint with an existing cache key and confirm byte-identical output vs the
  current code, then deploy to a scratch cache and validate a pulled NAR with
  `nix`. *Risk:* HIGH (signing path). Gate on the integration harness.
- **A4. Adopt shared `attic::api::v1` wire types** for `upload_path`,
  `get_missing_paths`, and `cache_config` (`CacheConfig`) instead of the local
  string-field shadows. Fix the two drifts this exposes: `retention_period` wire
  format (worker's `Option<Option<i32>>` vs core's `RetentionPeriodConfig` enum)
  and the two parallel `CompressionType`/`CompressionConfig` definitions in the
  worker. *Risk:* medium (stricter deserialization may reject previously-accepted
  malformed requests — that's a fix, but validate clients).

**Acceptance:** worker builds for wasm; the integration matrix passes; narinfo
bytes are unchanged on the wire.

---

## Workstream B — Dual-server admin (make the admin work against stock `atticd`)

This is a **feature**, not a cleanup — two tracks (admin side + atticd side).
Additive; non-breaking for the current Cloudflare deployment.

- **B1. Transport abstraction** (`admin/src/lib/server/attic-api.ts`). Introduce
  `AtticTransport` with two impls: `service-binding` (`env.ATTIC_API.fetch`) and
  `http+token` (`ATTIC_BASE_URL` + a long-lived `ATTIC_ADMIN_TOKEN` minted once
  with `atticadm`). Resolve by env. The JWT minting is already atticd-compatible.
- **B2. Repository abstraction for reads** (`CacheRepository`). Move every direct
  D1 read in the route loaders (`+page.server.ts`, `caches/**`, `store-paths.ts`,
  `monitoring`, `db/queries.ts`) behind it. `D1CacheRepository` keeps today's
  queries; `AtticdCacheRepository` calls REST. Backend feature-detected from env.
- **B3. New `atticd` endpoints** required for parity (in `server/src/api/v1`):
  - list caches (+ per-cache object count / bytes)
  - dashboard aggregate stats
  - store-path listing/search/sort (paginated) for a cache
  - monitoring time-series — or admit "unavailable against atticd" and degrade
  - `POST /_api/v1/gc` (wrap `run_garbage_collection_once`)
  - `POST /_api/v1/cache-config/:cache/rename` (mirror worker semantics: configure
    on source + create on destination)
  - add a `compression` column + `CacheConfig.compression` field + migration
    (atticd currently has no such column and silently drops the field)
  - token revocation: pick a shared story — a server-side denylist endpoint, or
    document TTL-only invalidation for atticd
- **B4. Graceful degradation.** A capabilities probe hides/disables worker-only
  controls (compression selector, monitoring, GC button, instant revocation) when
  the target backend lacks them.

**Acceptance:** an admin configured with `ATTIC_BASE_URL` + token renders
caches / paths / dashboard against a local `atticd`; create/configure/delete/
rename work; monitoring either works or degrades cleanly.

**Sequence:** B1 → B3 (endpoints, parallelizable) → B2 → B4.

---

## Workstream C — Worker-internal de-duplication (safe-ish, but critical path)

Real duplication, but on the upload path — land only behind the integration
harness.

- **C1.** `handle_buffered_upload` reads the body then calls
  `handle_buffered_upload_with_bytes` (collapses ~170 verbatim-duplicated lines).
- **C2.** Collapse `Stateful{Brotli,Gzip,Xz}Compressor` into one generic
  `StatefulCompressor<E: FinishableEncoder>` + a single `StatefulCompressionResult`;
  extract a `read_next_chunk` helper so the four stream-loop arms share one loop
  (~240 lines).
- **C3.** Merge `handle_deduplicated_upload` / `handle_chunked_deduplicated_upload`
  by unifying `UploadPathNarInfo` and `ChunkedNarInfo`.
- **C4.** One `compress_block(input, algo, level)` free fn shared by
  `compress_buffer` and `StreamingCompressor::compress_data`.
- **C5.** Shared GC/maintenance SQL constants across `d1.rs`/`turso.rs`
  (`?`-placeholder bodies; each backend renumbers): `purge_deleted_cache`,
  `delete_expired_objects`, `reap_orphan_nars`, `find_orphan_chunks`.

**Acceptance:** wasm build + full push/pull integration matrix; byte-identical
stored objects before/after.

---

## Workstream D — Feature-parity / GC gaps (both impls)

- **D1. GC abandoned soft-deleted caches.** Neither impl reclaims a cache that
  was soft-deleted and never reused — its NAR bytes sit in R2/S3 forever. Add a
  sweep to `worker/src/gc.rs` (and `server/src/gc.rs`): for each cache with
  `deleted_at < now - grace_period`, run the `purge_deleted_cache` cascade
  (objects → chunks → storage → cache row). This is a real storage leak; it's
  also what left `kclejeune` recoverable, so keep the grace window generous.
- **D2. Cache rename parity in `atticd`** (also listed under B3, tracked here as a
  standalone parity item).
- **D3.** Resolve dead `attic::api::v1::cache_config::supports_server_compression()`
  — either wire it into the client's compression gating (`client/src/push.rs`
  currently uploads uncompressed unconditionally) or drop it.

---

## Recommended order

1. **A1, A2** — safe shared-code consolidation, no harness needed.
2. **D1** — real storage leak, self-contained.
3. Build the **integration harness** (prereq).
4. **C1, C3, C5** (contained) then **C2, C4** (bigger) — behind the harness.
5. **A3, A4** — crypto/wire types, behind the harness + signature-parity check.
6. **Workstream B** — its own project: B1 → B3 → B2 → B4.
