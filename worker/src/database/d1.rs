//! Cloudflare D1 database backend.

use worker::d1::D1Database;

use super::models::*;
use crate::error::{WorkerError, WorkerResult};

/// D1 database backend using native Cloudflare bindings.
pub struct D1Backend {
    db: D1Database,
}

impl D1Backend {
    /// Create a new D1 backend.
    pub fn new(db: D1Database) -> Self {
        Self { db }
    }

    /// Find a cache by name.
    pub async fn find_cache(&self, name: &str) -> WorkerResult<Option<Cache>> {
        let stmt = self
            .db
            .prepare(
                "SELECT id, name, keypair, is_public, store_dir, priority, \
                 upstream_cache_key_names, compression, created_at, deleted_at, retention_period \
                 FROM cache WHERE name = ?1 AND deleted_at IS NULL",
            )
            .bind(&[name.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .first::<CacheRow>(None)
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(result.map(|row| row.into()))
    }

    /// Find an object by store path hash.
    pub async fn find_object(
        &self,
        cache_name: &str,
        store_path_hash: &str,
    ) -> WorkerResult<Option<ObjectWithNar>> {
        let stmt = self
            .db
            .prepare(
                "SELECT o.id, o.cache_id, o.nar_id, o.store_path_hash, o.store_path, \
                 o.refs, o.system, o.deriver, o.sigs, o.ca, o.created_at, \
                 o.last_accessed_at, o.created_by, \
                 n.id as nar_id2, n.state, n.nar_hash, n.nar_size, n.compression, \
                 n.num_chunks, n.completeness_hint, n.holders_count, n.created_at as nar_created_at \
                 FROM object o \
                 INNER JOIN cache c ON o.cache_id = c.id \
                 INNER JOIN nar n ON o.nar_id = n.id \
                 WHERE c.name = ?1 AND c.deleted_at IS NULL \
                 AND o.store_path_hash = ?2 AND n.state = 'V'",
            )
            .bind(&[cache_name.into(), store_path_hash.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .first::<ObjectWithNarRow>(None)
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(result.map(|row| row.into()))
    }

    /// Find a NAR by hash.
    pub async fn find_nar_by_hash(&self, nar_hash: &str) -> WorkerResult<Option<Nar>> {
        let stmt = self
            .db
            .prepare(
                "SELECT id, state, nar_hash, nar_size, compression, num_chunks, \
                 completeness_hint, holders_count, created_at \
                 FROM nar WHERE nar_hash = ?1 AND state = 'V'",
            )
            .bind(&[nar_hash.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .first::<NarRow>(None)
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(result.map(|row| row.into()))
    }

    /// Find chunks for a NAR.
    pub async fn find_chunks_for_nar(&self, nar_id: i64) -> WorkerResult<Vec<Chunk>> {
        let stmt = self
            .db
            .prepare(
                "SELECT ch.id, ch.state, ch.chunk_hash, ch.chunk_size, ch.file_hash, \
                 ch.file_size, ch.compression, ch.remote_file, ch.remote_file_id, \
                 ch.holders_count, ch.created_at \
                 FROM chunk ch \
                 INNER JOIN chunkref cr ON cr.chunk_id = ch.id \
                 WHERE cr.nar_id = ?1 \
                 ORDER BY cr.seq",
            )
            .bind(&[(nar_id as f64).into()]) // D1 doesn't support bigint
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let results = stmt
            .all()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let rows: Vec<ChunkRow> = results
            .results()
            .map_err(|e| WorkerError::Database(format!("Parse error: {}", e)))?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// Create a new cache.
    pub async fn create_cache(&self, cache: &Cache) -> WorkerResult<i64> {
        let stmt = self
            .db
            .prepare(
                "INSERT INTO cache (name, keypair, is_public, store_dir, priority, \
                 upstream_cache_key_names, compression, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT (name) DO NOTHING",
            )
            .bind(&[
                cache.name.clone().into(),
                cache.keypair.clone().into(),
                (cache.is_public as i32).into(),
                cache.store_dir.clone().into(),
                cache.priority.into(),
                serde_json::to_string(&cache.upstream_cache_key_names)
                    .unwrap_or_default()
                    .into(),
                cache.compression.clone().into(),
                cache.created_at.clone().into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;

        meta.and_then(|m| m.last_row_id)
            .map(|id| id as i64)
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a new NAR entry.
    pub async fn create_nar(&self, nar: &Nar) -> WorkerResult<i64> {
        let stmt = self
            .db
            .prepare(
                "INSERT INTO nar (state, nar_hash, nar_size, compression, num_chunks, \
                 completeness_hint, holders_count, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .bind(&[
                nar.state.as_str().into(),
                nar.nar_hash.clone().into(),
                (nar.nar_size as f64).into(), // D1 doesn't support bigint
                nar.compression.clone().into(),
                nar.num_chunks.into(),
                (nar.completeness_hint as i32).into(),
                nar.holders_count.into(),
                nar.created_at.clone().into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;

        meta.and_then(|m| m.last_row_id)
            .map(|id| id as i64)
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a new chunk entry.
    pub async fn create_chunk(&self, chunk: &Chunk) -> WorkerResult<i64> {
        let stmt = self
            .db
            .prepare(
                "INSERT INTO chunk (state, chunk_hash, chunk_size, file_hash, file_size, \
                 compression, remote_file, remote_file_id, holders_count, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .bind(&[
                chunk.state.as_str().into(),
                chunk.chunk_hash.clone().into(),
                (chunk.chunk_size as f64).into(), // D1 doesn't support bigint
                chunk
                    .file_hash
                    .clone()
                    .map(|s| s.into())
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
                chunk
                    .file_size
                    .map(|n| (n as f64).into()) // D1 doesn't support bigint
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
                chunk.compression.clone().into(),
                chunk.remote_file.clone().into(),
                chunk.remote_file_id.clone().into(),
                chunk.holders_count.into(),
                chunk.created_at.clone().into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;

        meta.and_then(|m| m.last_row_id)
            .map(|id| id as i64)
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a new object entry.
    pub async fn create_object(&self, object: &Object) -> WorkerResult<i64> {
        let stmt = self
            .db
            .prepare(
                "INSERT INTO object (cache_id, nar_id, store_path_hash, store_path, \
                 refs, system, deriver, sigs, ca, created_at, created_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
                 ON CONFLICT (cache_id, store_path_hash) DO UPDATE SET nar_id = excluded.nar_id",
            )
            .bind(&[
                (object.cache_id as f64).into(), // D1 doesn't support bigint
                (object.nar_id as f64).into(),   // D1 doesn't support bigint
                object.store_path_hash.clone().into(),
                object.store_path.clone().into(),
                serde_json::to_string(&object.references)
                    .unwrap_or_default()
                    .into(),
                object
                    .system
                    .clone()
                    .map(|s| s.into())
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
                object
                    .deriver
                    .clone()
                    .map(|s| s.into())
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
                serde_json::to_string(&object.sigs)
                    .unwrap_or_default()
                    .into(),
                object
                    .ca
                    .clone()
                    .map(|s| s.into())
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
                object.created_at.clone().into(),
                object
                    .created_by
                    .clone()
                    .map(|s| s.into())
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;

        meta.and_then(|m| m.last_row_id)
            .map(|id| id as i64)
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a chunk reference.
    pub async fn create_chunk_ref(&self, chunk_ref: &ChunkRef) -> WorkerResult<i64> {
        let stmt = self
            .db
            .prepare(
                "INSERT INTO chunkref (nar_id, seq, chunk_id, chunk_hash, compression) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(&[
                (chunk_ref.nar_id as f64).into(), // D1 doesn't support bigint
                chunk_ref.seq.into(),
                chunk_ref
                    .chunk_id
                    .map(|n| (n as f64).into()) // D1 doesn't support bigint
                    .unwrap_or(worker::wasm_bindgen::JsValue::NULL.into()),
                chunk_ref.chunk_hash.clone().into(),
                chunk_ref.compression.clone().into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;

        meta.and_then(|m| m.last_row_id)
            .map(|id| id as i64)
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Update NAR state.
    pub async fn update_nar_state(&self, nar_id: i64, state: NarState) -> WorkerResult<()> {
        let stmt = self
            .db
            .prepare("UPDATE nar SET state = ?1 WHERE id = ?2")
            .bind(&[state.as_str().into(), (nar_id as f64).into()]) // D1 doesn't support bigint
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        stmt.run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(())
    }

    /// Try to acquire a lock on a NAR for deduplication (optimistic locking).
    pub async fn try_lock_nar(&self, nar_hash: &str) -> WorkerResult<Option<Nar>> {
        // First, find a valid NAR with this hash
        let nar = match self.find_nar_by_hash(nar_hash).await? {
            Some(n) => n,
            None => return Ok(None),
        };

        let nar_id = nar
            .id
            .ok_or_else(|| WorkerError::Database("NAR has no ID".to_string()))?;

        // Try to atomically increment holders_count
        let stmt = self
            .db
            .prepare(
                "UPDATE nar SET holders_count = holders_count + 1 \
                 WHERE id = ?1 AND holders_count = ?2",
            )
            .bind(&[(nar_id as f64).into(), nar.holders_count.into()]) // D1 doesn't support bigint
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        // Check if we won the race
        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;
        let rows_affected = meta.and_then(|m| m.changes).unwrap_or(0) as u64;
        if rows_affected == 0 {
            // Someone else got it
            return Ok(None);
        }

        // Return the NAR with updated holders_count
        Ok(Some(Nar {
            holders_count: nar.holders_count + 1,
            ..nar
        }))
    }

    /// Release a NAR lock.
    pub async fn release_nar_lock(&self, nar_id: i64) -> WorkerResult<()> {
        let stmt = self
            .db
            .prepare("UPDATE nar SET holders_count = holders_count - 1 WHERE id = ?1 AND holders_count > 0")
            .bind(&[(nar_id as f64).into()]) // D1 doesn't support bigint
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        stmt.run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(())
    }

    /// Find which store path hashes already exist in a cache.
    ///
    /// Uses batched queries with SQL IN clauses to avoid exceeding D1's
    /// query limit per worker invocation (~1000 queries).
    pub async fn find_existing_paths(
        &self,
        cache_name: &str,
        hashes: &[String],
    ) -> WorkerResult<Vec<String>> {
        use worker::wasm_bindgen::JsValue;

        if hashes.is_empty() {
            return Ok(Vec::new());
        }

        // D1 has a limit of 100 bind parameters per query.
        // Reserve 1 for cache_name, so we can use up to 99 hashes per batch.
        const BATCH_SIZE: usize = 99;

        let mut existing = Vec::new();

        for batch in hashes.chunks(BATCH_SIZE) {
            // Build placeholders for IN clause: ?2, ?3, ?4, ...
            let placeholders: Vec<String> =
                (2..=batch.len() + 1).map(|i| format!("?{}", i)).collect();
            let placeholders_str = placeholders.join(", ");

            let query = format!(
                "SELECT o.store_path_hash FROM object o \
                 INNER JOIN cache c ON o.cache_id = c.id \
                 INNER JOIN nar n ON o.nar_id = n.id \
                 WHERE c.name = ?1 AND c.deleted_at IS NULL \
                 AND n.state = 'V' \
                 AND o.store_path_hash IN ({})",
                placeholders_str
            );

            // Build params: cache_name first, then all hashes in this batch
            let mut params: Vec<JsValue> = Vec::with_capacity(batch.len() + 1);
            params.push(cache_name.into());
            for hash in batch {
                params.push(hash.clone().into());
            }

            let stmt = self
                .db
                .prepare(&query)
                .bind(&params)
                .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

            let results = stmt
                .all()
                .await
                .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

            let rows: Vec<PathHashRow> = results
                .results()
                .map_err(|e| WorkerError::Database(format!("Parse error: {}", e)))?;

            for row in rows {
                existing.push(row.store_path_hash);
            }
        }

        Ok(existing)
    }

    /// Update cache configuration.
    pub async fn update_cache(
        &self,
        name: &str,
        is_public: Option<bool>,
        priority: Option<i32>,
        compression: Option<&str>,
        retention_period: Option<Option<i32>>,
        upstream_cache_key_names: Option<&[String]>,
        keypair: Option<&str>,
    ) -> WorkerResult<()> {
        use worker::wasm_bindgen::JsValue;

        // Build dynamic UPDATE query
        let mut updates = Vec::new();
        let mut params: Vec<JsValue> = Vec::new();
        let mut param_idx = 1;

        if let Some(val) = is_public {
            updates.push(format!("is_public = ?{}", param_idx));
            params.push(JsValue::from(val as i32));
            param_idx += 1;
        }

        if let Some(val) = priority {
            updates.push(format!("priority = ?{}", param_idx));
            params.push(JsValue::from(val));
            param_idx += 1;
        }

        if let Some(val) = compression {
            updates.push(format!("compression = ?{}", param_idx));
            params.push(JsValue::from_str(val));
            param_idx += 1;
        }

        if let Some(val) = retention_period {
            updates.push(format!("retention_period = ?{}", param_idx));
            match val {
                Some(n) => params.push(JsValue::from(n)),
                None => params.push(JsValue::NULL),
            }
            param_idx += 1;
        }

        if let Some(val) = upstream_cache_key_names {
            updates.push(format!("upstream_cache_key_names = ?{}", param_idx));
            let json = serde_json::to_string(val).unwrap_or_default();
            params.push(JsValue::from_str(&json));
            param_idx += 1;
        }

        if let Some(val) = keypair {
            updates.push(format!("keypair = ?{}", param_idx));
            params.push(JsValue::from_str(val));
            param_idx += 1;
        }

        if updates.is_empty() {
            return Ok(());
        }

        // Add the name parameter for WHERE clause
        params.push(JsValue::from_str(name));

        let query = format!(
            "UPDATE cache SET {} WHERE name = ?{} AND deleted_at IS NULL",
            updates.join(", "),
            param_idx
        );

        let stmt = self
            .db
            .prepare(&query)
            .bind(&params)
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        stmt.run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(())
    }

    /// Soft-delete a cache by setting deleted_at.
    ///
    /// Returns true if a cache was deleted, false if not found.
    pub async fn delete_cache(&self, name: &str) -> WorkerResult<bool> {
        let now = chrono::Utc::now().to_rfc3339();

        let stmt = self
            .db
            .prepare("UPDATE cache SET deleted_at = ?1 WHERE name = ?2 AND deleted_at IS NULL")
            .bind(&[now.into(), name.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;

        let rows_affected = meta.and_then(|m| m.changes).unwrap_or(0) as u64;
        Ok(rows_affected > 0)
    }

    /// Insert a new pending chunked-upload row.
    pub async fn create_pending_upload(&self, upload: &PendingUpload) -> WorkerResult<()> {
        let stmt = self
            .db
            .prepare(
                "INSERT INTO pending_upload (token, cache_id, cache_name, r2_upload_id, r2_key, \
                 storage_key, nar_info, expected_nar_size, compression, parts_uploaded, \
                 bytes_received, uploaded_parts, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )
            .bind(&[
                upload.token.clone().into(),
                (upload.cache_id as f64).into(),
                upload.cache_name.clone().into(),
                upload.r2_upload_id.clone().into(),
                upload.r2_key.clone().into(),
                upload.storage_key.clone().into(),
                upload.nar_info.clone().into(),
                (upload.expected_nar_size as f64).into(),
                upload.compression.clone().into(),
                upload.parts_uploaded.into(),
                (upload.bytes_received as f64).into(),
                upload.uploaded_parts.clone().into(),
                upload.created_at.clone().into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        stmt.run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(())
    }

    /// Fetch a pending chunked-upload row by token.
    pub async fn get_pending_upload(&self, token: &str) -> WorkerResult<Option<PendingUpload>> {
        let stmt = self
            .db
            .prepare(
                "SELECT token, cache_id, cache_name, r2_upload_id, r2_key, storage_key, \
                 nar_info, expected_nar_size, compression, parts_uploaded, bytes_received, \
                 uploaded_parts, created_at FROM pending_upload WHERE token = ?1",
            )
            .bind(&[token.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .first::<PendingUploadRow>(None)
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(result.map(|row| row.into()))
    }

    /// Update part accounting for a pending chunked-upload row.
    pub async fn update_pending_upload(
        &self,
        token: &str,
        parts_uploaded: i32,
        bytes_received: i64,
        uploaded_parts: &str,
    ) -> WorkerResult<()> {
        let stmt = self
            .db
            .prepare(
                "UPDATE pending_upload SET parts_uploaded = ?1, bytes_received = ?2, \
                 uploaded_parts = ?3 WHERE token = ?4",
            )
            .bind(&[
                parts_uploaded.into(),
                (bytes_received as f64).into(),
                uploaded_parts.into(),
                token.into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        stmt.run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(())
    }

    /// Delete a pending chunked-upload row by token.
    pub async fn delete_pending_upload(&self, token: &str) -> WorkerResult<()> {
        let stmt = self
            .db
            .prepare("DELETE FROM pending_upload WHERE token = ?1")
            .bind(&[token.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        stmt.run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(())
    }

    /// Create a pending device-authorization grant.
    pub async fn create_device_auth(
        &self,
        device_code: &str,
        user_code: &str,
        expires_at: i64,
    ) -> WorkerResult<()> {
        self.db
            .prepare(
                "INSERT INTO device_auth (device_code, user_code, status, created_at, expires_at) \
                 VALUES (?1, ?2, 'pending', ?3, ?4)",
            )
            .bind(&[
                device_code.into(),
                user_code.into(),
                (chrono::Utc::now().timestamp() as f64).into(),
                (expires_at as f64).into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;
        Ok(())
    }

    /// Look up a device grant by its device_code (for CLI polling).
    pub async fn find_device_auth(&self, device_code: &str) -> WorkerResult<Option<DeviceAuth>> {
        let stmt = self
            .db
            .prepare(
                "SELECT device_code, user_code, status, token, expires_at \
                 FROM device_auth WHERE device_code = ?1",
            )
            .bind(&[device_code.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;
        let row = stmt
            .first::<DeviceAuthRow>(None)
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;
        Ok(row.map(|r| r.into()))
    }

    /// Delete a device grant (after its token is retrieved, or on GC).
    pub async fn delete_device_auth(&self, device_code: &str) -> WorkerResult<()> {
        self.db
            .prepare("DELETE FROM device_auth WHERE device_code = ?1")
            .bind(&[device_code.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;
        Ok(())
    }

    /// GC: delete device grants past their expiry.
    pub async fn delete_expired_device_auth(&self) -> WorkerResult<u64> {
        let result = self
            .db
            .prepare("DELETE FROM device_auth WHERE expires_at < ?1")
            .bind(&[(chrono::Utc::now().timestamp() as f64).into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;
        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;
        Ok(meta.and_then(|m| m.changes).unwrap_or(0) as u64)
    }

    /// Delete objects whose cache has a retention period and which have aged out
    /// (by last access, falling back to creation time). Returns rows deleted.
    pub async fn delete_expired_objects(&self) -> WorkerResult<u64> {
        let result = self
            .db
            .prepare(
                "DELETE FROM object WHERE id IN (\
                 SELECT o.id FROM object o JOIN cache c ON c.id = o.cache_id \
                 WHERE c.retention_period IS NOT NULL AND c.deleted_at IS NULL \
                 AND datetime(COALESCE(o.last_accessed_at, o.created_at)) < \
                     datetime('now', '-' || c.retention_period || ' days'))",
            )
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;
        Ok(meta.and_then(|m| m.changes).unwrap_or(0) as u64)
    }

    /// Reap NARs (and their chunk references) no longer referenced by any object,
    /// past a one-hour grace period so in-flight uploads are left alone.
    pub async fn reap_orphan_nars(&self) -> WorkerResult<u64> {
        self.db
            .prepare(
                "DELETE FROM chunkref WHERE nar_id IN (\
                 SELECT n.id FROM nar n \
                 WHERE NOT EXISTS (SELECT 1 FROM object o WHERE o.nar_id = n.id) \
                 AND datetime(n.created_at) < datetime('now', '-1 hours'))",
            )
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let result = self
            .db
            .prepare(
                "DELETE FROM nar WHERE \
                 NOT EXISTS (SELECT 1 FROM object o WHERE o.nar_id = nar.id) \
                 AND datetime(created_at) < datetime('now', '-1 hours')",
            )
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let meta = result
            .meta()
            .map_err(|e| WorkerError::Database(format!("Meta error: {}", e)))?;
        Ok(meta.and_then(|m| m.changes).unwrap_or(0) as u64)
    }

    /// Chunks no longer referenced by any chunkref (their R2 bytes can be freed).
    pub async fn find_orphan_chunks(&self) -> WorkerResult<Vec<OrphanChunk>> {
        let result = self
            .db
            .prepare(
                "SELECT id, remote_file FROM chunk \
                 WHERE NOT EXISTS (SELECT 1 FROM chunkref cr WHERE cr.chunk_id = chunk.id)",
            )
            .all()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let rows = result
            .results::<OrphanChunkRow>()
            .map_err(|e| WorkerError::Database(format!("Deserialize error: {}", e)))?;
        Ok(rows
            .into_iter()
            .map(|r| OrphanChunk {
                id: r.id,
                remote_file: r.remote_file,
            })
            .collect())
    }

    /// Delete a chunk row by id.
    pub async fn delete_chunk(&self, id: i64) -> WorkerResult<()> {
        self.db
            .prepare("DELETE FROM chunk WHERE id = ?1")
            .bind(&[(id as f64).into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;
        Ok(())
    }

    /// Record an access time for an object, for LRU-based retention.
    pub async fn touch_object(&self, cache_name: &str, store_path_hash: &str) -> WorkerResult<()> {
        self.db
            .prepare(
                "UPDATE object SET last_accessed_at = ?1 \
                 WHERE store_path_hash = ?2 \
                 AND cache_id = (SELECT id FROM cache WHERE name = ?3)",
            )
            .bind(&[
                chrono::Utc::now().to_rfc3339().into(),
                store_path_hash.into(),
                cache_name.into(),
            ])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?
            .run()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;
        Ok(())
    }

    /// Whether an admin-issued token (by `jti`) has been revoked.
    ///
    /// Returns false when no matching row exists (the token is not admin-tracked,
    /// e.g. a bootstrap token) so only explicitly revoked tokens are rejected.
    pub async fn is_token_revoked(&self, jti: &str) -> WorkerResult<bool> {
        let stmt = self
            .db
            .prepare("SELECT revoked_at FROM api_token WHERE id = ?1")
            .bind(&[jti.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let row = stmt
            .first::<RevokedRow>(None)
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        Ok(row.and_then(|r| r.revoked_at).is_some())
    }

    /// List pending uploads created before the given RFC3339 timestamp (for GC).
    pub async fn list_stale_pending_uploads(
        &self,
        before: &str,
    ) -> WorkerResult<Vec<PendingUpload>> {
        let stmt = self
            .db
            .prepare(
                "SELECT token, cache_id, cache_name, r2_upload_id, r2_key, storage_key, \
                 nar_info, expected_nar_size, compression, parts_uploaded, bytes_received, \
                 uploaded_parts, created_at FROM pending_upload WHERE created_at < ?1",
            )
            .bind(&[before.into()])
            .map_err(|e| WorkerError::Database(format!("Bind error: {}", e)))?;

        let result = stmt
            .all()
            .await
            .map_err(|e| WorkerError::Database(format!("Query error: {}", e)))?;

        let rows = result
            .results::<PendingUploadRow>()
            .map_err(|e| WorkerError::Database(format!("Deserialize error: {}", e)))?;

        Ok(rows.into_iter().map(|row| row.into()).collect())
    }
}

// Row types for D1 deserialization
#[derive(serde::Deserialize)]
struct CacheRow {
    id: Option<i64>,
    name: String,
    keypair: String,
    is_public: i32,
    store_dir: String,
    priority: i32,
    upstream_cache_key_names: String,
    compression: String,
    created_at: String,
    deleted_at: Option<String>,
    retention_period: Option<i32>,
}

impl From<CacheRow> for Cache {
    fn from(row: CacheRow) -> Self {
        Cache {
            id: row.id,
            name: row.name,
            keypair: row.keypair,
            is_public: row.is_public != 0,
            store_dir: row.store_dir,
            priority: row.priority,
            upstream_cache_key_names: serde_json::from_str(&row.upstream_cache_key_names)
                .unwrap_or_default(),
            compression: row.compression,
            created_at: row.created_at,
            deleted_at: row.deleted_at,
            retention_period: row.retention_period,
        }
    }
}

#[derive(serde::Deserialize)]
struct NarRow {
    id: Option<i64>,
    state: String,
    nar_hash: String,
    nar_size: i64,
    compression: String,
    num_chunks: i32,
    completeness_hint: i32,
    holders_count: i32,
    created_at: String,
}

impl From<NarRow> for Nar {
    fn from(row: NarRow) -> Self {
        Nar {
            id: row.id,
            state: NarState::from_str(&row.state).unwrap_or(NarState::Valid),
            nar_hash: row.nar_hash,
            nar_size: row.nar_size,
            compression: row.compression,
            num_chunks: row.num_chunks,
            completeness_hint: row.completeness_hint != 0,
            holders_count: row.holders_count,
            created_at: row.created_at,
        }
    }
}

#[derive(serde::Deserialize)]
struct ChunkRow {
    id: Option<i64>,
    state: String,
    chunk_hash: String,
    chunk_size: i64,
    file_hash: Option<String>,
    file_size: Option<i64>,
    compression: String,
    remote_file: String,
    remote_file_id: String,
    holders_count: i32,
    created_at: String,
}

impl From<ChunkRow> for Chunk {
    fn from(row: ChunkRow) -> Self {
        Chunk {
            id: row.id,
            state: ChunkState::from_str(&row.state).unwrap_or(ChunkState::Valid),
            chunk_hash: row.chunk_hash,
            chunk_size: row.chunk_size,
            file_hash: row.file_hash,
            file_size: row.file_size,
            compression: row.compression,
            remote_file: row.remote_file,
            remote_file_id: row.remote_file_id,
            holders_count: row.holders_count,
            created_at: row.created_at,
        }
    }
}

#[derive(serde::Deserialize)]
struct ObjectWithNarRow {
    id: Option<i64>,
    cache_id: i64,
    nar_id: i64,
    store_path_hash: String,
    store_path: String,
    refs: String,
    system: Option<String>,
    deriver: Option<String>,
    sigs: String,
    ca: Option<String>,
    created_at: String,
    last_accessed_at: Option<String>,
    created_by: Option<String>,
    // NAR fields
    #[allow(dead_code)]
    nar_id2: Option<i64>,
    state: String,
    nar_hash: String,
    nar_size: i64,
    compression: String,
    num_chunks: i32,
    completeness_hint: i32,
    holders_count: i32,
    nar_created_at: String,
}

impl From<ObjectWithNarRow> for ObjectWithNar {
    fn from(row: ObjectWithNarRow) -> Self {
        ObjectWithNar {
            object: Object {
                id: row.id,
                cache_id: row.cache_id,
                nar_id: row.nar_id,
                store_path_hash: row.store_path_hash,
                store_path: row.store_path,
                references: serde_json::from_str(&row.refs).unwrap_or_default(),
                system: row.system,
                deriver: row.deriver,
                sigs: serde_json::from_str(&row.sigs).unwrap_or_default(),
                ca: row.ca,
                created_at: row.created_at,
                last_accessed_at: row.last_accessed_at,
                created_by: row.created_by,
            },
            nar: Nar {
                id: Some(row.nar_id),
                state: NarState::from_str(&row.state).unwrap_or(NarState::Valid),
                nar_hash: row.nar_hash,
                nar_size: row.nar_size,
                compression: row.compression,
                num_chunks: row.num_chunks,
                completeness_hint: row.completeness_hint != 0,
                holders_count: row.holders_count,
                created_at: row.nar_created_at,
            },
        }
    }
}

#[derive(serde::Deserialize)]
struct PathHashRow {
    store_path_hash: String,
}

#[derive(serde::Deserialize)]
struct RevokedRow {
    revoked_at: Option<i64>,
}

#[derive(serde::Deserialize)]
struct OrphanChunkRow {
    id: i64,
    remote_file: String,
}

#[derive(serde::Deserialize)]
struct DeviceAuthRow {
    device_code: String,
    user_code: String,
    status: String,
    token: Option<String>,
    expires_at: i64,
}

impl From<DeviceAuthRow> for DeviceAuth {
    fn from(r: DeviceAuthRow) -> Self {
        DeviceAuth {
            device_code: r.device_code,
            user_code: r.user_code,
            status: r.status,
            token: r.token,
            expires_at: r.expires_at,
        }
    }
}

#[derive(serde::Deserialize)]
struct PendingUploadRow {
    token: String,
    cache_id: i64,
    cache_name: String,
    r2_upload_id: String,
    r2_key: String,
    storage_key: String,
    nar_info: String,
    expected_nar_size: i64,
    compression: String,
    parts_uploaded: i32,
    bytes_received: i64,
    uploaded_parts: String,
    created_at: String,
}

impl From<PendingUploadRow> for PendingUpload {
    fn from(row: PendingUploadRow) -> Self {
        PendingUpload {
            token: row.token,
            cache_id: row.cache_id,
            cache_name: row.cache_name,
            r2_upload_id: row.r2_upload_id,
            r2_key: row.r2_key,
            storage_key: row.storage_key,
            nar_info: row.nar_info,
            expected_nar_size: row.expected_nar_size,
            compression: row.compression,
            parts_uploaded: row.parts_uploaded,
            bytes_received: row.bytes_received,
            uploaded_parts: row.uploaded_parts,
            created_at: row.created_at,
        }
    }
}
