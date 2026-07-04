//! Garbage collection for the Attic Worker.
//!
//! Runs on a scheduled (cron) trigger. Cloudflare Workers cannot hold long-lived
//! background state, so GC is a set of idempotent sweeps that each reclaim a
//! bounded amount of work per invocation.

use serde::Serialize;
use worker::console_log;

use crate::state::WorkerState;

/// Age after which an in-progress chunked upload is considered abandoned.
const ABANDONED_UPLOAD_MAX_AGE_SECS: i64 = 24 * 60 * 60;

/// Result of a GC run, for logging and the admin trigger response.
#[derive(Debug, Default, Serialize)]
pub struct GcStats {
    pub abandoned_uploads_reaped: u64,
    pub abandoned_upload_errors: u64,
    pub expired_objects_reaped: u64,
    pub orphan_nars_reaped: u64,
    pub orphan_chunks_reaped: u64,
}

/// Run all garbage-collection sweeps.
///
/// Ordered so each pass exposes work for the next: retention deletes objects,
/// which orphans their NARs, which orphans their chunks.
pub async fn run(state: &WorkerState) -> GcStats {
    let mut stats = GcStats::default();

    reap_abandoned_uploads(state, &mut stats).await;
    reap_expired_objects(state, &mut stats).await;
    reap_orphans(state, &mut stats).await;

    // Expired device-authorization grants (best-effort).
    if let Err(e) = state.database.delete_expired_device_auth().await {
        console_log!("gc: device_auth cleanup failed: {}", e);
    }

    stats
}

/// Time-based retention: drop objects that have aged out of their cache's window.
async fn reap_expired_objects(state: &WorkerState, stats: &mut GcStats) {
    match state.database.delete_expired_objects().await {
        Ok(n) => stats.expired_objects_reaped = n,
        Err(e) => console_log!("gc: retention pass failed: {}", e),
    }
}

/// Reap NARs no longer referenced by any object, then the chunks they freed
/// (deleting the chunk bytes from R2).
async fn reap_orphans(state: &WorkerState, stats: &mut GcStats) {
    match state.database.reap_orphan_nars().await {
        Ok(n) => stats.orphan_nars_reaped = n,
        Err(e) => console_log!("gc: orphan NAR reap failed: {}", e),
    }

    let orphans = match state.database.find_orphan_chunks().await {
        Ok(c) => c,
        Err(e) => {
            console_log!("gc: find orphan chunks failed: {}", e);
            return;
        }
    };

    for chunk in orphans {
        if let Ok(remote) = serde_json::from_str::<serde_json::Value>(&chunk.remote_file) {
            if let Some(key) = remote.get("key").and_then(|v| v.as_str()) {
                if let Err(e) = state.storage.delete_file(key).await {
                    console_log!("gc: failed to delete R2 object {}: {}", key, e);
                }
            }
        }
        match state.database.delete_chunk(chunk.id).await {
            Ok(()) => stats.orphan_chunks_reaped += 1,
            Err(e) => console_log!("gc: failed to delete chunk {}: {}", chunk.id, e),
        }
    }
}

/// Reap chunked uploads that were started but never completed.
///
/// Each leaves an open R2 multipart upload (which R2 never expires on its own)
/// plus a `pending_upload` row. We abort the R2 upload and delete the row.
async fn reap_abandoned_uploads(state: &WorkerState, stats: &mut GcStats) {
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(ABANDONED_UPLOAD_MAX_AGE_SECS))
        .to_rfc3339();

    let stale = match state.database.list_stale_pending_uploads(&cutoff).await {
        Ok(rows) => rows,
        Err(e) => {
            console_log!("gc: failed to list stale pending uploads: {}", e);
            return;
        }
    };

    for upload in stale {
        // Abort the R2 multipart upload. Best-effort: it may already be gone.
        match state
            .storage
            .resume_multipart_upload(
                &upload.r2_key,
                &upload.r2_upload_id,
                upload.parts_uploaded as u16,
            )
            .await
        {
            Ok(multipart) => {
                if let Err(e) = multipart.abort().await {
                    console_log!("gc: failed to abort multipart {}: {}", upload.r2_key, e);
                }
            }
            Err(e) => {
                console_log!(
                    "gc: failed to resume multipart {} for abort: {}",
                    upload.r2_key,
                    e
                );
            }
        }

        // Delete the tracking row regardless; a dangling row is worse than a
        // possibly-already-aborted R2 upload.
        match state.database.delete_pending_upload(&upload.token).await {
            Ok(()) => stats.abandoned_uploads_reaped += 1,
            Err(e) => {
                stats.abandoned_upload_errors += 1;
                console_log!("gc: failed to delete pending_upload row: {}", e);
            }
        }
    }
}
