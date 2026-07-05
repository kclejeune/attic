# Consolidation & dual-server — implementation guide

A step-by-step guide for an engineer/subagent to finish the worker↔server
consolidation and dual-server admin work flagged in
[`followups-consolidation.md`](./followups-consolidation.md). Each task lists
its **files**, **steps**, **verification**, **risk**, and **acceptance**. Do the
workstreams in the order in §7. Read §0 and §1 before touching anything.

Status of prior work (already landed on `kcl/worker-impl`):
- Worker GC reaps abandoned soft-deleted caches (`worker/src/gc.rs`).
- atticd GC parity (`server/src/gc.rs::run_reap_abandoned_caches`).
- Delete stays soft; create/rename purge a soft-deleted tombstone
  (`worker` cache-config handlers + `purge_deleted_cache`).
- Store-path list has sort/search/infinite-scroll + `idx_object_cache_created`
  / `idx_object_cache_path` indexes.

---

## 0. Environment & invariants (read first)

**Repo layout.** Rust workspace crates: `attic/` (core, wasm-safe pieces + native
`nix_store` via CXX), `attic-token` (`token/`), `attic-client` (`client/`),
`attic-server` (`server/`). The Cloudflare Worker (`worker/`) is a **separate
crate excluded from the workspace** with its **own `Cargo.lock`**, targeting
`wasm32-unknown-unknown`. The SvelteKit admin is `admin/`.

**Build / check / deploy commands.**
| What | Command | Notes |
|---|---|---|
| Worker type-check | `cd worker && nix develop ..#default -c cargo check --target wasm32-unknown-unknown` | pre-existing `user_code` dead-code warning is expected |
| Worker deploy | `cd worker && npx wrangler deploy` | **Run OUTSIDE `nix develop`.** The nix shell shadows `~/.cargo/bin/worker-build` (0.1.14) with a 0.8.5 build that rejects `worker = 0.4.2`. The wrangler build command is `npm install && cargo install -q worker-build@0.1.14 && worker-build --release`. `wasm-bindgen` is pinned `=0.2.105` in `worker/Cargo.toml`. |
| Server check/test | `nix develop -c cargo check -p attic-server` / `... cargo test -p attic-server` | needs nix for libnixstore/pkg-config/curl/git2/sodium/blake3/brotli |
| attic core test | `nix develop -c cargo test -p attic` | host toolchain; **this is where shared-code parity tests run** |
| Admin | `cd admin && npm run check && npm run build && npx wrangler deploy` | live at `app.cache.kclj.io` (Cloudflare Access gated — you cannot browse it headless) |

**Live infra (production — mutate carefully).** Worker `attic-worker` at
`cache.kclj.io`; admin `attic-admin` at `app.cache.kclj.io`; D1 database named
`attic`, id `872ceec3-5438-4069-b3fa-69fbb66dd011`; R2 bucket `attic-cache`;
admin→worker service binding `ATTIC_API`. Only one real cache: `kclejeune`
(~7,941 objects). D1 can be queried/mutated with the Cloudflare D1 MCP tool or
`wrangler d1 execute`.

**Foreign keys.** D1 **enforces** FKs but the worker schema has **no
`ON DELETE CASCADE`** → always delete children before parents (object →
pending_upload → cache; chunkref before nar; etc.). atticd's schema **does** have
`ON DELETE CASCADE` on `object → cache` and `chunkref → {nar,chunk}`, so deleting
a cache row there cascades its objects.

**⚠️ THE SIGNING INVARIANT (most important).** A cache's narinfo carries a
`Sig:` line = ed25519 over the *fingerprint*
`1;{storePath};{narHash-nix-base32};{narSize};{comma-joined-full-refs}`. **Any
byte drift in the fingerprint, hash encoding, or signing changes the signature
and breaks verification for every already-pushed path.** Before deploying ANY
change under Workstream A or C to the live worker:
1. Capture a golden narinfo for a known path, e.g.
   `curl -s https://cache.kclj.io/kclejeune/gvabsb4yqb5xsqzqph54rijnn4zpihnp.narinfo`
   and save the `URL:`, `Compression:`, `FileHash:`, `NarHash:`, and `Sig:` lines.
2. After deploy, re-fetch and diff. **`Sig:` and every field MUST be identical.**
   If not, roll back immediately (`git revert` + redeploy).

**Keypair / crypto facts.** Keypair string format `{name}:{base64(secret32 ‖
public32)}`. The worker (`worker/src/crypto.rs`) uses `ed25519-dalek`; `attic`
core (`attic/src/signing/`) uses `ed25519-compact` (wasm-friendly; the worker
already enables `getrandom/js`). Both must produce byte-identical signatures for
the same key+message (ed25519 is deterministic, so they will — but prove it, see
A3).

---

## 1. Verification harness (prerequisite for Workstreams A & C)

You cannot run the worker crate's `#[test]`s on the host (its `worker`-crate deps
are wasm-only). Two complementary harnesses make A/C safe:

**1a. Host parity tests in `attic` core (required, cheap).** Every shared
function A/C introduces (`compute_fingerprint`, hash conversion, signing) gets a
`#[test]` in `attic/` that locks its **exact byte/string output** against known
vectors. Seed vectors from the worker's current tests
(`worker/src/crypto.rs` `mod tests`: `test_compute_fingerprint`,
`test_convert_hash_to_base32`, `test_sign_message`). Because the worker will call
the *same* `attic` code, passing these locks the worker's output too. Run with
`nix develop -c cargo test -p attic`.

**1b. Live golden-narinfo diff (required before every worker deploy in A/C).** As
in §0 — capture `.narinfo` for a known path before, diff after. This is the
integration check that the deployed wasm actually produces identical output.

**1c. Full push/pull harness (optional, needed to be fully confident on C's
upload path).** Options, easiest first:
- **Scratch cache on the live worker.** Mint an admin-scoped token
  (`atticadm make-token` with create/push/pull, or via the admin), create a
  throwaway cache, `attic push` a small path, `attic` pull / `nix copy` it back,
  verify the signature, then delete the cache. Least infra, uses real R2.
- **Miniflare.** `wrangler dev` with local D1 + R2; drive the same push/pull. More
  isolation, more setup; workerd must run the built wasm.
Exercise the matrix `{buffered <15MB, streaming >15MB, chunked, dedup} ×
{none, zstd, br, xz, gzip}` and assert pulled bytes + signature verify.

---

## 2. Workstream A — worker ↔ attic-core consolidation

Goal: the worker reuses `attic` core instead of re-implementing crypto/hash/wire
types. Do A1→A2→A4→A3 (A3, swapping the ed25519 lib, is highest-risk; do it last
behind the fullest verification). One worker deploy can carry several once each
is host-tested.

### A1 — Share `compute_fingerprint`

**Files:** `attic/src/signing/mod.rs` (or a new `attic/src/signing/fingerprint.rs`
re-exported from `signing`); `worker/src/crypto.rs`; `server/src/narinfo/mod.rs`.

**Current duplication.** `worker/src/crypto.rs:118` `compute_fingerprint(store_path,
nar_hash, nar_size, references)` and `server/src/narinfo/mod.rs` `NarInfo::fingerprint`
implement the identical `1;…;…;…;…` format. The worker version: takes a base32-or-hex
`nar_hash` (calls `convert_hash_to_base32` internally — see A2), and for each
reference, uses it as-is if it starts with `/`, else prepends the store dir
(parent of `store_path`). The server version: uses `nar_hash.to_typed_base32()`,
`self.store_path.as_os_str().as_bytes()` (unix), and always prepends `store_dir` to
each (base-name) reference.

**Steps:**
1. Add a `std`-only, wasm-safe free fn to `attic` core:
   ```rust
   /// 1;{store_path};{nar_hash_base32};{nar_size};{comma-joined full references}
   /// `nar_hash_base32` must already be typed nix-base32 (e.g. "sha256:1abc…").
   /// `full_references` are complete store paths (callers prefix the store dir).
   pub fn compute_fingerprint<S: AsRef<str>>(
       store_path: &str, nar_hash_base32: &str, nar_size: u64, full_references: &[S],
   ) -> Vec<u8>
   ```
   Body is the byte assembly only (no hash conversion, no ref prefixing).
2. Worker `crypto.rs`: keep the store-dir-prefixing + `convert_hash_to_base32`
   (from A2) at the call site, build the `Vec<String>` of full refs and the
   base32 hash, then call `attic::signing::compute_fingerprint`. Delete the
   worker's format loop.
3. Server `NarInfo::fingerprint`: build full refs `format!("{}/{}", store_dir, r)`,
   pass `self.store_path.to_str().expect("store path is UTF-8")` (Nix store paths
   are always UTF-8) and `self.nar_hash.to_typed_base32()`, call the shared fn.
   (If you want to preserve the theoretical non-UTF-8 path, instead make the
   shared fn take `store_path: &[u8]` and `full_references: &[Vec<u8>]`; the
   worker passes `.as_bytes()`. The `&str` form is simpler and safe for Nix.)

**Verify:** move the worker's `test_compute_fingerprint` vector into `attic`'s
tests and add the server's expected fingerprint for a known `NarInfo`; `cargo test
-p attic` + `cargo check -p attic-server`; worker wasm check. Then §1b golden diff.

**Risk:** medium (touches signing input). **Acceptance:** worker + server produce
byte-identical fingerprints to before; golden `Sig:` unchanged.

### A2 — Share hash base32 conversion

**Files:** `worker/src/crypto.rs`, `worker/src/handlers/binary_cache.rs` (call
sites ~L426/446).

`worker/src/crypto.rs:170` `convert_hash_to_base32(h)` reimplements
`attic::hash::Hash::from_typed(h)?.to_typed_base32()`. Replace the body with:
```rust
attic::hash::Hash::from_typed(hash)
    .map(|h| h.to_typed_base32())
    .unwrap_or_else(|_| hash.to_string()) // preserve worker's passthrough-on-error
```
**Verify:** parity test in `attic` (or worker vector reused) asserting equality
for a hex sha256, a base32 sha256, and a malformed input (must pass through
unchanged); §1b golden diff. **Risk:** low. **Acceptance:** identical output for
all DB-stored hashes (always valid), passthrough preserved for the invalid case.

### A3 — Share signing keypair (delete worker crypto, drop `ed25519-dalek`)

**Files:** `worker/src/crypto.rs`, `worker/Cargo.toml`, all call sites of
`generate_keypair`/`extract_public_key`/`sign_message`.

Replace with `attic::signing::NixKeypair`:
`generate_keypair(name)` → `NixKeypair::generate(name)?.export_keypair()`;
`extract_public_key(kp)` → `NixKeypair::from_str(kp)?.export_public_key()`;
`sign_message(kp, msg)` → `NixKeypair::from_str(kp)?.sign(msg)`. Remove
`ed25519-dalek` from `worker/Cargo.toml`; delete the corresponding `crypto.rs`
functions (keep `compute_fingerprint`/`convert_hash_to_base32` thin wrappers or
inline).

**Verify (critical):** add an `attic` test that signs a **fixed** keypair over a
**fixed** fingerprint and asserts the exact base64 signature equals what the
worker's `ed25519-dalek` path produces today (capture that value first from a
throwaway wasm run or by signing the same key+message with a small `ed25519-dalek`
snippet). Deterministic ed25519 ⇒ identical; prove it. Then §1c full push to a
scratch cache and verify `nix` accepts the signature, and §1b golden diff on
`kclejeune`. **Risk:** HIGH. Do last, behind §1c. **Acceptance:** signatures
byte-identical; a freshly pushed path verifies under `nix`.

### A4 — Share API wire types + unify compression enums

**Files:** `worker/src/handlers/v1/upload_path.rs` (local `UploadPathNarInfo`
L27, `ChunkedNarInfo` L1209, `StartChunkedUploadRequest/Response`,
`CompleteChunkedUploadRequest`, `UploadPathResult` L41), `worker/src/handlers/v1/
get_missing_paths.rs` (L11), `worker/src/handlers/v1/cache_config.rs` (local
`CacheConfigUpdate` L22), `worker/src/compression/mod.rs` (second `CompressionType`).

Replace the local string-field structs with `attic::api::v1::upload_path::*`,
`attic::api::v1::get_missing_paths::*`, and `attic::api::v1::cache_config::CacheConfig`.
Unwrap the newtypes at use sites (`CacheName::as_str()`, `Hash`, `StorePathHash`).
**Behavioral note:** the shared types validate on deserialize (`Hash`, `CacheName`),
so malformed requests that the worker currently accepts will now 400 — that's a
fix, but confirm the CLI/clients send valid values. **`retention_period` drift:**
the worker uses `Option<Option<i32>>`; the shared `CacheConfig` uses
`Option<RetentionPeriodConfig>` (`Global | Period(u32)`). Adopt the enum and
update the worker's DB write + the admin's PATCH body
(`admin/.../settings/+page.server.ts`) to match, or you break retention config.
Collapse the worker's two `CompressionType` enums to `attic::compression::CompressionType`.

**Verify:** worker wasm check; a create + configure + get round-trip on a scratch
cache; confirm retention still persists. **Risk:** medium. **Acceptance:** wire
format unchanged for valid inputs; retention config still works.

---

## 3. Workstream C — worker-internal de-duplication

All touch the upload path — **land only behind §1c**. No behavior change intended;
verify with §1c matrix + §1b.

- **C1.** `worker/src/handlers/v1/upload_path.rs`: `handle_buffered_upload`
  (~L830) reads the body then calls `handle_buffered_upload_with_bytes` (~L1010);
  delete the ~170 duplicated lines from the former.
- **C2.** `worker/src/compression/streaming.rs`: collapse
  `Stateful{Brotli,Gzip,Xz}Compressor` into one `StatefulCompressor<E:
  FinishableEncoder>` (`write_bytes`, `finish_into_collector`) + a single
  `StatefulCompressionResult`. Extract `read_next_chunk(reader) -> Option<Vec<u8>>`
  so the four stream-loop arms in `upload_path.rs` (~L374/448/521/602) share one
  `while let Some(chunk) = read_next_chunk(&reader).await? { … }`.
- **C3.** Merge `handle_deduplicated_upload` / `handle_chunked_deduplicated_upload`
  by unifying `UploadPathNarInfo`/`ChunkedNarInfo` (dovetails with A4).
- **C4.** One `compress_block(input, algo, level) -> Vec<u8>` shared by
  `stream.rs::compress_buffer` and `streaming.rs::StreamingCompressor::compress_data`.
- **C5.** Shared GC/maintenance SQL constants across `d1.rs`/`turso.rs`
  (`purge_deleted_cache`, `reap_abandoned_caches`, `delete_expired_objects`,
  `reap_orphan_nars`, `find_orphan_chunks`) — a `mod sql` with `?` placeholders;
  D1 renumbers to `?N`, Turso keeps `?`. (Do NOT try to share the row→model
  conversion or the dynamic `update_cache` builder — the typed-serde vs
  positional-JSON APIs make that not worth it; see the simplify audit's
  non-findings.)

**Acceptance:** §1c matrix passes; stored objects byte-identical; §1b golden diff.

---

## 4. Workstream B — dual-server admin (works against stock atticd)

A feature, not a cleanup. Additive; never touches the worker signing path. Two
tracks: admin side (transport + repository) and atticd side (new endpoints).
Order: B1 → B3 (parallelizable) → B2 → B4.

### B1 — Transport abstraction

**File:** `admin/src/lib/server/attic-api.ts`. Today `atticFetch` hardcodes
`env.ATTIC_API.fetch(...)` (a CF service binding). Introduce:
```ts
interface AtticTransport { fetch(path: string, init?: RequestInit): Promise<Response>; }
function resolveTransport(env: Env): AtticTransport {
  if (env.ATTIC_API) return { fetch: (p, i) =>
    env.ATTIC_API.fetch(new Request(`https://attic-api${p}`, i) as never) as unknown as Response };
  const base = env.ATTIC_BASE_URL?.replace(/\/$/, ''); const token = env.ATTIC_ADMIN_TOKEN;
  if (base && token) return { fetch: (p, i) => {
    const h = new Headers(i?.headers); h.set('Authorization', `Bearer ${token}`);
    return fetch(`${base}${p}`, { ...i, headers: h }); } };
  throw new Error('No attic transport configured');
}
```
In atticd mode `ATTIC_ADMIN_TOKEN` is a long-lived admin JWT minted once with
`atticadm make-token` (the JWT minting in `attic-token.ts` already produces
atticd-compatible HS256 tokens; keep server-side minting only for the CF path).
Add `ATTIC_BASE_URL` / `ATTIC_ADMIN_TOKEN` to `admin/wrangler.jsonc` vars/secrets
and `App.Platform['env']` types. **Acceptance:** create/configure/delete a cache
against a local atticd via `ATTIC_BASE_URL`.

### B2 — Repository abstraction for reads

**Files:** new `admin/src/lib/server/repository/{index,d1,atticd}.ts`; refactor
every direct-D1 read to go through it:
`admin/src/routes/(app)/+page.server.ts` (dashboard stats),
`caches/+page.server.ts` (list + per-cache size), `caches/[name]/+page.server.ts`,
`caches/[name]/paths/+server.ts` + `lib/server/store-paths.ts`,
`caches/[name]/settings/+page.server.ts` (read), `monitoring/+page.server.ts`,
`lib/server/db/queries.ts`.
```ts
interface CacheRepository {
  listCaches(): Promise<CacheSummary[]>;
  getCache(name: string): Promise<CacheDetail | null>;
  listPaths(cache: string, opts: PathQueryOpts): Promise<{ paths: StorePath[]; hasMore: boolean }>;
  countPaths(cache: string, q: string): Promise<number>;
  getStats(): Promise<DashboardStats>;
  getSeries(opts: SeriesOpts): Promise<Bucket[]>; // may throw NotSupported
}
```
`D1CacheRepository` = today's queries (move `store-paths.ts` + the inline loaders
in). `AtticdCacheRepository` = REST via the B1 transport hitting the B3 endpoints.
Select by env (presence of `ATTIC_DB` ⇒ D1, else atticd). **Acceptance:** all
list/browse/dashboard pages render against atticd (monitoring may degrade — B4).

### B3 — New atticd endpoints (`server/src/api/v1/`)

Add to the router in `server/src/api/v1/mod.rs` (currently only GET/POST/PATCH/
DELETE on `/_api/v1/cache-config/{cache}`). Mirror worker semantics/permissions.
- `GET /_api/v1/cache-list` → `[{name, is_public, priority, compression?, retention,
  objects, bytes}]`. Guard: any-cache read or list a caller's caches.
- `GET /_api/v1/stats` → dashboard counts (caches, objects, nars, storage bytes,
  pending, orphan nars/chunks) filtered to non-deleted caches.
- `GET /_api/v1/cache-config/{cache}/paths?sort&dir&q&offset&limit` → paginated
  store paths (mirror `worker` store-paths query). Guard: pull on the cache.
- `GET /_api/v1/monitoring?granularity&range` → zero-filled time-series (mirror
  `admin/monitoring/+page.server.ts` bucketing) — or skip and let B4 degrade.
- `POST /_api/v1/gc` → call `run_garbage_collection_once(config)`, return stats.
  Guard: wildcard delete (probe a synthetic cache like the worker's `gc.rs`).
- `POST /_api/v1/cache-config/{cache}/rename` (also **D2**) → mirror the worker:
  configure on source + create on destination; under `soft_delete_caches` purge a
  soft-deleted tombstone at the destination first, keep the keypair. See
  `worker/src/handlers/v1/cache_config.rs::rename_cache`.
- **compression column (also finding #5):** add `compression: Option<String>` to
  atticd's `cache` entity + a migration + `CacheConfig` field, else the admin's
  compression control is silently dropped against atticd.
- **token revocation:** either add `POST /_api/v1/tokens/revoke` + a server-side
  denylist check in `server/src/access/`, or document TTL-only invalidation for
  atticd and hide instant-revoke in the admin (B4).

**Verify:** `cargo test -p attic-server`; hit each endpoint against a local atticd
(sqlite is fine) with an admin token. **Acceptance:** each returns the shape
`AtticdCacheRepository` expects.

### B4 — Capability degradation

**Files:** a capabilities probe (e.g. `GET /_api/v1/cache-config/{any}` already
returns `worker_capabilities` for the worker; add a `server_kind`/capabilities
field to atticd's config response) surfaced via `event.locals` or a `+layout`
load. Hide/disable controls the target backend lacks: compression selector,
monitoring page, GC button, instant token revocation. **Acceptance:** no dead
buttons when pointed at an atticd that lacks a capability.

---

## 5. Workstream D — remaining parity

- **D2. atticd rename** — folded into B3 (the rename endpoint). Standalone if B is
  deferred.
- **D3. Dead code.** `attic/src/api/v1/cache_config.rs::supports_server_compression()`
  has no caller. Either wire it into the client's compression gating
  (`client/src/push.rs` currently uploads uncompressed unconditionally — gate on
  it) or delete it.

---

## 6. Do-not-do list (from the simplify audit's non-findings)

- Do **not** try to unify the `d1.rs` row→model structs with `turso.rs` positional
  parsers, or share the two backends' dynamic `update_cache` query builders — the
  typed-serde vs positional-JSON-array APIs make a shared form more complex than
  the duplication. C5 (static SQL strings) is the only worthwhile backend dedup.
- Do **not** factor the admin's `if (!locals.user) throw error(401)` guards into a
  helper — SvelteKit actions are plain functions with no shared context; the
  repetition is idiomatic.

---

## 7. Recommended order & gating

1. **B (dual-server)** — highest user-visible value, **zero risk to the live
   signing path**. Ship B1 → B3 → B2 → B4. Recommended to do first.
2. **Harness §1a/§1b** — cheap; needed before any A/C worker deploy.
3. **A1, A2, A4** — behind §1a + §1b golden diff.
4. **§1c full push/pull harness** — before A3 and C.
5. **A3** (ed25519 swap) and **C1–C5** — behind §1c.
6. **D3** — anytime (trivial).

**Every worker deploy under A/C:** capture the golden narinfo (§0), deploy, diff.
If `Sig:` or any narinfo field changed, `git revert` + redeploy and stop.
