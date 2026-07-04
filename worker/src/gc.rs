//! Garbage collection for the Attic Worker.
//!
//! Runs on a scheduled (cron) trigger. Cloudflare Workers cannot hold long-lived
//! background state, so GC is a set of idempotent sweeps that each reclaim a
//! bounded amount of work per invocation.

use worker::console_log;

use crate::state::WorkerState;

/// Age after which an in-progress chunked upload is considered abandoned.
const ABANDONED_UPLOAD_MAX_AGE_SECS: i64 = 24 * 60 * 60;

/// Result of a GC run, for logging.
#[derive(Debug, Default)]
pub struct GcStats {
    pub abandoned_uploads_reaped: u64,
    pub abandoned_upload_errors: u64,
}

/// Run all garbage-collection sweeps.
pub async fn run(state: &WorkerState) -> GcStats {
    let mut stats = GcStats::default();

    reap_abandoned_uploads(state, &mut stats).await;

    stats
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
