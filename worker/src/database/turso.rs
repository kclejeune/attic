//! Turso/libSQL database backend via HTTP API.

use serde::{Deserialize, Serialize};

use super::models::*;
use crate::error::{WorkerError, WorkerResult};

/// Turso database backend using the HTTP API.
pub struct TursoBackend {
    url: String,
    auth_token: String,
}

/// Turso HTTP API request.
#[derive(Serialize)]
struct TursoRequest {
    statements: Vec<TursoStatement>,
}

/// A single SQL statement.
#[derive(Serialize)]
struct TursoStatement {
    q: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Vec<serde_json::Value>>,
}

/// Turso HTTP API response.
#[derive(Deserialize)]
struct TursoResponse {
    results: Vec<TursoResult>,
}

/// A single result.
#[derive(Deserialize)]
struct TursoResult {
    #[allow(dead_code)]
    columns: Option<Vec<String>>,
    rows: Option<Vec<Vec<serde_json::Value>>>,
    rows_affected: Option<u64>,
    last_insert_rowid: Option<i64>,
}

impl TursoBackend {
    /// Create a new Turso backend.
    pub fn new(url: String, auth_token: String) -> Self {
        Self { url, auth_token }
    }

    /// Execute a query and return raw results.
    ///
    /// Uses the Workers runtime `Fetch` API. (The DOM `fetch` via
    /// `web_sys::window()` is unavailable in Workers — there is no `window`.)
    async fn execute(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> WorkerResult<TursoResult> {
        use worker::{Fetch, Headers, Method, Request, RequestInit};

        let request = TursoRequest {
            statements: vec![TursoStatement {
                q: sql.to_string(),
                params: if params.is_empty() {
                    None
                } else {
                    Some(params)
                },
            }],
        };

        let body = serde_json::to_string(&request)
            .map_err(|e| WorkerError::Database(format!("JSON error: {}", e)))?;

        let mut headers = Headers::new();
        headers
            .set("Content-Type", "application/json")
            .map_err(|e| WorkerError::Internal(format!("Headers error: {:?}", e)))?;
        headers
            .set("Authorization", &format!("Bearer {}", self.auth_token))
            .map_err(|e| WorkerError::Internal(format!("Headers error: {:?}", e)))?;

        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(wasm_bindgen::JsValue::from_str(&body)));

        let url = format!("{}/v2/pipeline", self.url);
        let req = Request::new_with_init(&url, &init)
            .map_err(|e| WorkerError::Internal(format!("Request error: {:?}", e)))?;

        let mut resp = Fetch::Request(req)
            .send()
            .await
            .map_err(|e| WorkerError::Database(format!("Fetch error: {:?}", e)))?;

        let status = resp.status_code();
        if !(200..300).contains(&status) {
            return Err(WorkerError::Database(format!(
                "Turso API error: {}",
                status
            )));
        }

        let response: TursoResponse = resp
            .json()
            .await
            .map_err(|e| WorkerError::Database(format!("JSON parse error: {:?}", e)))?;

        response
            .results
            .into_iter()
            .next()
            .ok_or_else(|| WorkerError::Database("No result from Turso".to_string()))
    }

    /// Find a cache by name.
    pub async fn find_cache(&self, name: &str) -> WorkerResult<Option<Cache>> {
        let result = self
            .execute(
                "SELECT id, name, keypair, is_public, store_dir, priority, \
                 upstream_cache_key_names, compression, created_at, deleted_at, retention_period \
                 FROM cache WHERE name = ? AND deleted_at IS NULL",
                vec![serde_json::Value::String(name.to_string())],
            )
            .await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(parse_cache_row(&row)?));
            }
        }

        Ok(None)
    }

    /// Find an object by store path hash.
    pub async fn find_object(
        &self,
        cache_name: &str,
        store_path_hash: &str,
    ) -> WorkerResult<Option<ObjectWithNar>> {
        let result = self
            .execute(
                "SELECT o.id, o.cache_id, o.nar_id, o.store_path_hash, o.store_path, \
                 o.refs, o.system, o.deriver, o.sigs, o.ca, o.created_at, \
                 o.last_accessed_at, o.created_by, \
                 n.id, n.state, n.nar_hash, n.nar_size, n.compression, \
                 n.num_chunks, n.completeness_hint, n.holders_count, n.created_at \
                 FROM object o \
                 INNER JOIN cache c ON o.cache_id = c.id \
                 INNER JOIN nar n ON o.nar_id = n.id \
                 WHERE c.name = ? AND c.deleted_at IS NULL \
                 AND o.store_path_hash = ? AND n.state = 'V'",
                vec![
                    serde_json::Value::String(cache_name.to_string()),
                    serde_json::Value::String(store_path_hash.to_string()),
                ],
            )
            .await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(parse_object_with_nar_row(&row)?));
            }
        }

        Ok(None)
    }

    /// Find a NAR by hash.
    pub async fn find_nar_by_hash(&self, nar_hash: &str) -> WorkerResult<Option<Nar>> {
        let result = self
            .execute(
                "SELECT id, state, nar_hash, nar_size, compression, num_chunks, \
                 completeness_hint, holders_count, created_at \
                 FROM nar WHERE nar_hash = ? AND state = 'V'",
                vec![serde_json::Value::String(nar_hash.to_string())],
            )
            .await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(parse_nar_row(&row)?));
            }
        }

        Ok(None)
    }

    /// Find chunks for a NAR.
    pub async fn find_chunks_for_nar(&self, nar_id: i64) -> WorkerResult<Vec<Chunk>> {
        let result = self
            .execute(
                "SELECT ch.* FROM chunk ch \
                 INNER JOIN chunkref cr ON cr.chunk_id = ch.id \
                 WHERE cr.nar_id = ? \
                 ORDER BY cr.seq",
                vec![serde_json::Value::Number(nar_id.into())],
            )
            .await?;

        let mut chunks = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                chunks.push(parse_chunk_row(&row)?);
            }
        }

        Ok(chunks)
    }

    /// Create a new cache.
    pub async fn create_cache(&self, cache: &Cache) -> WorkerResult<i64> {
        let result = self
            .execute(
                "INSERT INTO cache (name, keypair, is_public, store_dir, priority, \
                 upstream_cache_key_names, compression, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (name) DO NOTHING \
                 RETURNING id",
                vec![
                    serde_json::Value::String(cache.name.clone()),
                    serde_json::Value::String(cache.keypair.clone()),
                    serde_json::Value::Bool(cache.is_public),
                    serde_json::Value::String(cache.store_dir.clone()),
                    serde_json::Value::Number(cache.priority.into()),
                    serde_json::Value::String(
                        serde_json::to_string(&cache.upstream_cache_key_names).unwrap_or_default(),
                    ),
                    serde_json::Value::String(cache.compression.clone()),
                    serde_json::Value::String(cache.created_at.clone()),
                ],
            )
            .await?;

        result
            .last_insert_rowid
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a new NAR entry.
    pub async fn create_nar(&self, nar: &Nar) -> WorkerResult<i64> {
        let result = self
            .execute(
                "INSERT INTO nar (state, nar_hash, nar_size, compression, num_chunks, \
                 completeness_hint, holders_count, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id",
                vec![
                    serde_json::Value::String(nar.state.as_str().to_string()),
                    serde_json::Value::String(nar.nar_hash.clone()),
                    serde_json::Value::Number(nar.nar_size.into()),
                    serde_json::Value::String(nar.compression.clone()),
                    serde_json::Value::Number(nar.num_chunks.into()),
                    serde_json::Value::Bool(nar.completeness_hint),
                    serde_json::Value::Number(nar.holders_count.into()),
                    serde_json::Value::String(nar.created_at.clone()),
                ],
            )
            .await?;

        result
            .last_insert_rowid
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a new chunk entry.
    pub async fn create_chunk(&self, chunk: &Chunk) -> WorkerResult<i64> {
        let result = self
            .execute(
                "INSERT INTO chunk (state, chunk_hash, chunk_size, file_hash, file_size, \
                 compression, remote_file, remote_file_id, holders_count, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 RETURNING id",
                vec![
                    serde_json::Value::String(chunk.state.as_str().to_string()),
                    serde_json::Value::String(chunk.chunk_hash.clone()),
                    serde_json::Value::Number(chunk.chunk_size.into()),
                    chunk
                        .file_hash
                        .as_ref()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .unwrap_or(serde_json::Value::Null),
                    chunk
                        .file_size
                        .map(|n| serde_json::Value::Number(n.into()))
                        .unwrap_or(serde_json::Value::Null),
                    serde_json::Value::String(chunk.compression.clone()),
                    serde_json::Value::String(chunk.remote_file.clone()),
                    serde_json::Value::String(chunk.remote_file_id.clone()),
                    serde_json::Value::Number(chunk.holders_count.into()),
                    serde_json::Value::String(chunk.created_at.clone()),
                ],
            )
            .await?;

        result
            .last_insert_rowid
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a new object entry.
    pub async fn create_object(&self, object: &Object) -> WorkerResult<i64> {
        let result = self
            .execute(
                "INSERT INTO object (cache_id, nar_id, store_path_hash, store_path, \
                 refs, system, deriver, sigs, ca, created_at, created_by) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (cache_id, store_path_hash) DO UPDATE SET nar_id = excluded.nar_id \
                 RETURNING id",
                vec![
                    serde_json::Value::Number(object.cache_id.into()),
                    serde_json::Value::Number(object.nar_id.into()),
                    serde_json::Value::String(object.store_path_hash.clone()),
                    serde_json::Value::String(object.store_path.clone()),
                    serde_json::Value::String(
                        serde_json::to_string(&object.references).unwrap_or_default(),
                    ),
                    object
                        .system
                        .as_ref()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .unwrap_or(serde_json::Value::Null),
                    object
                        .deriver
                        .as_ref()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .unwrap_or(serde_json::Value::Null),
                    serde_json::Value::String(
                        serde_json::to_string(&object.sigs).unwrap_or_default(),
                    ),
                    object
                        .ca
                        .as_ref()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .unwrap_or(serde_json::Value::Null),
                    serde_json::Value::String(object.created_at.clone()),
                    object
                        .created_by
                        .as_ref()
                        .map(|s| serde_json::Value::String(s.clone()))
                        .unwrap_or(serde_json::Value::Null),
                ],
            )
            .await?;

        result
            .last_insert_rowid
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Create a chunk reference.
    pub async fn create_chunk_ref(&self, chunk_ref: &ChunkRef) -> WorkerResult<i64> {
        let result = self
            .execute(
                "INSERT INTO chunkref (nar_id, seq, chunk_id, chunk_hash, compression) \
                 VALUES (?, ?, ?, ?, ?) \
                 RETURNING id",
                vec![
                    serde_json::Value::Number(chunk_ref.nar_id.into()),
                    serde_json::Value::Number(chunk_ref.seq.into()),
                    chunk_ref
                        .chunk_id
                        .map(|n| serde_json::Value::Number(n.into()))
                        .unwrap_or(serde_json::Value::Null),
                    serde_json::Value::String(chunk_ref.chunk_hash.clone()),
                    serde_json::Value::String(chunk_ref.compression.clone()),
                ],
            )
            .await?;

        result
            .last_insert_rowid
            .ok_or_else(|| WorkerError::Database("No row ID returned from insert".to_string()))
    }

    /// Update NAR state.
    pub async fn update_nar_state(&self, nar_id: i64, state: NarState) -> WorkerResult<()> {
        self.execute(
            "UPDATE nar SET state = ? WHERE id = ?",
            vec![
                serde_json::Value::String(state.as_str().to_string()),
                serde_json::Value::Number(nar_id.into()),
            ],
        )
        .await?;

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
        let result = self
            .execute(
                "UPDATE nar SET holders_count = holders_count + 1 \
                 WHERE id = ? AND holders_count = ?",
                vec![
                    serde_json::Value::Number(nar_id.into()),
                    serde_json::Value::Number(nar.holders_count.into()),
                ],
            )
            .await?;

        // Check if we won the race
        if result.rows_affected.unwrap_or(0) == 0 {
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
        self.execute(
            "UPDATE nar SET holders_count = holders_count - 1 WHERE id = ? AND holders_count > 0",
            vec![serde_json::Value::Number(nar_id.into())],
        )
        .await?;

        Ok(())
    }

    /// Find which store path hashes already exist in a cache.
    pub async fn find_existing_paths(
        &self,
        cache_name: &str,
        hashes: &[String],
    ) -> WorkerResult<Vec<String>> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }

        // Build placeholders for IN clause
        let placeholders: Vec<String> = hashes.iter().map(|_| "?".to_string()).collect();
        let query = format!(
            "SELECT o.store_path_hash FROM object o \
             INNER JOIN cache c ON o.cache_id = c.id \
             INNER JOIN nar n ON o.nar_id = n.id \
             WHERE c.name = ? AND c.deleted_at IS NULL \
             AND n.state = 'V' \
             AND o.store_path_hash IN ({})",
            placeholders.join(", ")
        );

        let mut params: Vec<serde_json::Value> =
            vec![serde_json::Value::String(cache_name.to_string())];
        params.extend(hashes.iter().map(|h| serde_json::Value::String(h.clone())));

        let result = self.execute(&query, params).await?;

        let mut existing = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                if let Some(serde_json::Value::String(hash)) = row.first() {
                    existing.push(hash.clone());
                }
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
        // Build dynamic UPDATE query
        let mut updates = Vec::new();
        let mut params: Vec<serde_json::Value> = Vec::new();

        if let Some(val) = is_public {
            updates.push("is_public = ?");
            params.push(serde_json::Value::Bool(val));
        }

        if let Some(val) = priority {
            updates.push("priority = ?");
            params.push(serde_json::Value::Number(val.into()));
        }

        if let Some(val) = compression {
            updates.push("compression = ?");
            params.push(serde_json::Value::String(val.to_string()));
        }

        if let Some(val) = retention_period {
            updates.push("retention_period = ?");
            match val {
                Some(n) => params.push(serde_json::Value::Number(n.into())),
                None => params.push(serde_json::Value::Null),
            }
        }

        if let Some(val) = upstream_cache_key_names {
            updates.push("upstream_cache_key_names = ?");
            let json = serde_json::to_string(val).unwrap_or_default();
            params.push(serde_json::Value::String(json));
        }

        if let Some(val) = keypair {
            updates.push("keypair = ?");
            params.push(serde_json::Value::String(val.to_string()));
        }

        if updates.is_empty() {
            return Ok(());
        }

        // Add the name parameter for WHERE clause
        params.push(serde_json::Value::String(name.to_string()));

        let query = format!(
            "UPDATE cache SET {} WHERE name = ? AND deleted_at IS NULL",
            updates.join(", ")
        );

        self.execute(&query, params).await?;

        Ok(())
    }

    /// Soft-delete a cache by setting deleted_at.
    ///
    /// Returns true if a cache was deleted, false if not found.
    pub async fn delete_cache(&self, name: &str) -> WorkerResult<bool> {
        let now = chrono::Utc::now().to_rfc3339();

        let result = self
            .execute(
                "UPDATE cache SET deleted_at = ? WHERE name = ? AND deleted_at IS NULL",
                vec![
                    serde_json::Value::String(now),
                    serde_json::Value::String(name.to_string()),
                ],
            )
            .await?;

        Ok(result.rows_affected.unwrap_or(0) > 0)
    }

    /// Insert a new pending chunked-upload row.
    pub async fn create_pending_upload(&self, upload: &PendingUpload) -> WorkerResult<()> {
        self.execute(
            "INSERT INTO pending_upload (token, cache_id, cache_name, r2_upload_id, r2_key, \
             storage_key, nar_info, expected_nar_size, compression, parts_uploaded, \
             bytes_received, uploaded_parts, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                serde_json::Value::String(upload.token.clone()),
                serde_json::Value::Number(upload.cache_id.into()),
                serde_json::Value::String(upload.cache_name.clone()),
                serde_json::Value::String(upload.r2_upload_id.clone()),
                serde_json::Value::String(upload.r2_key.clone()),
                serde_json::Value::String(upload.storage_key.clone()),
                serde_json::Value::String(upload.nar_info.clone()),
                serde_json::Value::Number(upload.expected_nar_size.into()),
                serde_json::Value::String(upload.compression.clone()),
                serde_json::Value::Number(upload.parts_uploaded.into()),
                serde_json::Value::Number(upload.bytes_received.into()),
                serde_json::Value::String(upload.uploaded_parts.clone()),
                serde_json::Value::String(upload.created_at.clone()),
            ],
        )
        .await?;

        Ok(())
    }

    /// Fetch a pending chunked-upload row by token.
    pub async fn get_pending_upload(&self, token: &str) -> WorkerResult<Option<PendingUpload>> {
        let result = self
            .execute(
                "SELECT token, cache_id, cache_name, r2_upload_id, r2_key, storage_key, \
                 nar_info, expected_nar_size, compression, parts_uploaded, bytes_received, \
                 uploaded_parts, created_at FROM pending_upload WHERE token = ?",
                vec![serde_json::Value::String(token.to_string())],
            )
            .await?;

        if let Some(rows) = result.rows {
            if let Some(row) = rows.into_iter().next() {
                return Ok(Some(parse_pending_upload_row(&row)?));
            }
        }

        Ok(None)
    }

    /// Update part accounting for a pending chunked-upload row.
    pub async fn update_pending_upload(
        &self,
        token: &str,
        parts_uploaded: i32,
        bytes_received: i64,
        uploaded_parts: &str,
    ) -> WorkerResult<()> {
        self.execute(
            "UPDATE pending_upload SET parts_uploaded = ?, bytes_received = ?, \
             uploaded_parts = ? WHERE token = ?",
            vec![
                serde_json::Value::Number(parts_uploaded.into()),
                serde_json::Value::Number(bytes_received.into()),
                serde_json::Value::String(uploaded_parts.to_string()),
                serde_json::Value::String(token.to_string()),
            ],
        )
        .await?;

        Ok(())
    }

    /// Delete a pending chunked-upload row by token.
    pub async fn delete_pending_upload(&self, token: &str) -> WorkerResult<()> {
        self.execute(
            "DELETE FROM pending_upload WHERE token = ?",
            vec![serde_json::Value::String(token.to_string())],
        )
        .await?;

        Ok(())
    }

    /// List pending uploads created before the given RFC3339 timestamp (for GC).
    pub async fn list_stale_pending_uploads(
        &self,
        before: &str,
    ) -> WorkerResult<Vec<PendingUpload>> {
        let result = self
            .execute(
                "SELECT token, cache_id, cache_name, r2_upload_id, r2_key, storage_key, \
                 nar_info, expected_nar_size, compression, parts_uploaded, bytes_received, \
                 uploaded_parts, created_at FROM pending_upload WHERE created_at < ?",
                vec![serde_json::Value::String(before.to_string())],
            )
            .await?;

        let mut uploads = Vec::new();
        if let Some(rows) = result.rows {
            for row in rows {
                uploads.push(parse_pending_upload_row(&row)?);
            }
        }

        Ok(uploads)
    }
}

/// Parse a pending upload row from query results.
fn parse_pending_upload_row(row: &[serde_json::Value]) -> WorkerResult<PendingUpload> {
    let s = |i: usize| {
        row.get(i)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Ok(PendingUpload {
        token: s(0),
        cache_id: row.get(1).and_then(|v| v.as_i64()).unwrap_or_default(),
        cache_name: s(2),
        r2_upload_id: s(3),
        r2_key: s(4),
        storage_key: s(5),
        nar_info: s(6),
        expected_nar_size: row.get(7).and_then(|v| v.as_i64()).unwrap_or_default(),
        compression: s(8),
        parts_uploaded: row.get(9).and_then(|v| v.as_i64()).unwrap_or_default() as i32,
        bytes_received: row.get(10).and_then(|v| v.as_i64()).unwrap_or_default(),
        uploaded_parts: {
            let v = s(11);
            if v.is_empty() {
                "[]".to_string()
            } else {
                v
            }
        },
        created_at: s(12),
    })
}

/// Parse a joined object+nar row (object columns 0-12, nar columns 13-21).
fn parse_object_with_nar_row(row: &[serde_json::Value]) -> WorkerResult<ObjectWithNar> {
    let s = |i: usize| {
        row.get(i)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let opt_s = |i: usize| row.get(i).and_then(|v| v.as_str()).map(|s| s.to_string());
    let json_list = |i: usize| {
        row.get(i)
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    };

    Ok(ObjectWithNar {
        object: Object {
            id: row.get(0).and_then(|v| v.as_i64()),
            cache_id: row.get(1).and_then(|v| v.as_i64()).unwrap_or_default(),
            nar_id: row.get(2).and_then(|v| v.as_i64()).unwrap_or_default(),
            store_path_hash: s(3),
            store_path: s(4),
            references: json_list(5),
            system: opt_s(6),
            deriver: opt_s(7),
            sigs: json_list(8),
            ca: opt_s(9),
            created_at: s(10),
            last_accessed_at: opt_s(11),
            created_by: opt_s(12),
        },
        nar: Nar {
            id: row.get(13).and_then(|v| v.as_i64()),
            state: row
                .get(14)
                .and_then(|v| v.as_str())
                .and_then(NarState::from_str)
                .unwrap_or(NarState::Valid),
            nar_hash: s(15),
            nar_size: row.get(16).and_then(|v| v.as_i64()).unwrap_or_default(),
            compression: row
                .get(17)
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string(),
            num_chunks: row.get(18).and_then(|v| v.as_i64()).unwrap_or(1) as i32,
            completeness_hint: row.get(19).and_then(|v| v.as_bool()).unwrap_or(false),
            holders_count: row.get(20).and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            created_at: s(21),
        },
    })
}

/// Parse a cache row from query results.
fn parse_cache_row(row: &[serde_json::Value]) -> WorkerResult<Cache> {
    Ok(Cache {
        id: row.get(0).and_then(|v| v.as_i64()),
        name: row
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        keypair: row
            .get(2)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        is_public: row.get(3).and_then(|v| v.as_bool()).unwrap_or(false),
        store_dir: row
            .get(4)
            .and_then(|v| v.as_str())
            .unwrap_or("/nix/store")
            .to_string(),
        priority: row.get(5).and_then(|v| v.as_i64()).unwrap_or(40) as i32,
        upstream_cache_key_names: row
            .get(6)
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default(),
        compression: row
            .get(7)
            .and_then(|v| v.as_str())
            .unwrap_or("br")
            .to_string(),
        created_at: row
            .get(8)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        deleted_at: row.get(9).and_then(|v| v.as_str()).map(|s| s.to_string()),
        retention_period: row.get(10).and_then(|v| v.as_i64()).map(|n| n as i32),
    })
}

/// Parse a NAR row from query results.
fn parse_nar_row(row: &[serde_json::Value]) -> WorkerResult<Nar> {
    Ok(Nar {
        id: row.get(0).and_then(|v| v.as_i64()),
        state: row
            .get(1)
            .and_then(|v| v.as_str())
            .and_then(NarState::from_str)
            .unwrap_or(NarState::Valid),
        nar_hash: row
            .get(2)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        nar_size: row.get(3).and_then(|v| v.as_i64()).unwrap_or(0),
        compression: row
            .get(4)
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string(),
        num_chunks: row.get(5).and_then(|v| v.as_i64()).unwrap_or(1) as i32,
        completeness_hint: row.get(6).and_then(|v| v.as_bool()).unwrap_or(true),
        holders_count: row.get(7).and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        created_at: row
            .get(8)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// Parse a chunk row from query results.
fn parse_chunk_row(row: &[serde_json::Value]) -> WorkerResult<Chunk> {
    Ok(Chunk {
        id: row.get(0).and_then(|v| v.as_i64()),
        state: row
            .get(1)
            .and_then(|v| v.as_str())
            .and_then(ChunkState::from_str)
            .unwrap_or(ChunkState::Valid),
        chunk_hash: row
            .get(2)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        chunk_size: row.get(3).and_then(|v| v.as_i64()).unwrap_or(0),
        file_hash: row.get(4).and_then(|v| v.as_str()).map(|s| s.to_string()),
        file_size: row.get(5).and_then(|v| v.as_i64()),
        compression: row
            .get(6)
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string(),
        remote_file: row
            .get(7)
            .and_then(|v| v.as_str())
            .unwrap_or("{}")
            .to_string(),
        remote_file_id: row
            .get(8)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        holders_count: row.get(9).and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        created_at: row
            .get(10)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}
