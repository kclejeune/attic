//! Database backends for the Attic Worker.

mod d1;
mod models;
mod turso;

pub use d1::D1Backend;
pub use models::*;
pub use turso::TursoBackend;

use worker::Env;

use crate::error::{WorkerError, WorkerResult};

/// Database backend abstraction.
pub enum Database {
    /// Cloudflare D1 (native SQLite).
    D1(D1Backend),
    /// Turso/libSQL over HTTP.
    Turso(TursoBackend),
}

impl Database {
    /// Create a database backend from the environment.
    pub fn from_env(env: &Env) -> WorkerResult<Self> {
        // Try D1 first (native, faster)
        if let Ok(d1) = env.d1("ATTIC_DB") {
            return Ok(Database::D1(D1Backend::new(d1)));
        }

        // Fall back to Turso
        if let (Ok(url), Ok(token)) = (env.var("TURSO_URL"), env.secret("TURSO_AUTH_TOKEN")) {
            return Ok(Database::Turso(TursoBackend::new(
                url.to_string(),
                token.to_string(),
            )));
        }

        Err(WorkerError::Configuration(
            "No database configuration found. Configure D1 binding 'ATTIC_DB', or set TURSO_URL and TURSO_AUTH_TOKEN."
                .to_string(),
        ))
    }

    /// Find a cache by name.
    pub async fn find_cache(&self, name: &str) -> WorkerResult<Option<Cache>> {
        match self {
            Database::D1(backend) => backend.find_cache(name).await,
            Database::Turso(backend) => backend.find_cache(name).await,
        }
    }

    /// Find an object by store path hash.
    pub async fn find_object(
        &self,
        cache_name: &str,
        store_path_hash: &str,
    ) -> WorkerResult<Option<ObjectWithNar>> {
        match self {
            Database::D1(backend) => backend.find_object(cache_name, store_path_hash).await,
            Database::Turso(backend) => backend.find_object(cache_name, store_path_hash).await,
        }
    }

    /// Find a NAR by hash.
    pub async fn find_nar_by_hash(&self, nar_hash: &str) -> WorkerResult<Option<Nar>> {
        match self {
            Database::D1(backend) => backend.find_nar_by_hash(nar_hash).await,
            Database::Turso(backend) => backend.find_nar_by_hash(nar_hash).await,
        }
    }

    /// Find chunks for a NAR.
    pub async fn find_chunks_for_nar(&self, nar_id: i64) -> WorkerResult<Vec<Chunk>> {
        match self {
            Database::D1(backend) => backend.find_chunks_for_nar(nar_id).await,
            Database::Turso(backend) => backend.find_chunks_for_nar(nar_id).await,
        }
    }

    /// Create a new cache.
    pub async fn create_cache(&self, cache: &Cache) -> WorkerResult<i64> {
        match self {
            Database::D1(backend) => backend.create_cache(cache).await,
            Database::Turso(backend) => backend.create_cache(cache).await,
        }
    }

    /// Create a new NAR entry.
    pub async fn create_nar(&self, nar: &Nar) -> WorkerResult<i64> {
        match self {
            Database::D1(backend) => backend.create_nar(nar).await,
            Database::Turso(backend) => backend.create_nar(nar).await,
        }
    }

    /// Create a new chunk entry.
    pub async fn create_chunk(&self, chunk: &Chunk) -> WorkerResult<i64> {
        match self {
            Database::D1(backend) => backend.create_chunk(chunk).await,
            Database::Turso(backend) => backend.create_chunk(chunk).await,
        }
    }

    /// Create a new object entry.
    pub async fn create_object(&self, object: &Object) -> WorkerResult<i64> {
        match self {
            Database::D1(backend) => backend.create_object(object).await,
            Database::Turso(backend) => backend.create_object(object).await,
        }
    }

    /// Create a chunk reference.
    pub async fn create_chunk_ref(&self, chunk_ref: &ChunkRef) -> WorkerResult<i64> {
        match self {
            Database::D1(backend) => backend.create_chunk_ref(chunk_ref).await,
            Database::Turso(backend) => backend.create_chunk_ref(chunk_ref).await,
        }
    }

    /// Update NAR state.
    pub async fn update_nar_state(&self, nar_id: i64, state: NarState) -> WorkerResult<()> {
        match self {
            Database::D1(backend) => backend.update_nar_state(nar_id, state).await,
            Database::Turso(backend) => backend.update_nar_state(nar_id, state).await,
        }
    }

    /// Try to acquire a lock on a NAR for deduplication (optimistic locking).
    pub async fn try_lock_nar(&self, nar_hash: &str) -> WorkerResult<Option<Nar>> {
        match self {
            Database::D1(backend) => backend.try_lock_nar(nar_hash).await,
            Database::Turso(backend) => backend.try_lock_nar(nar_hash).await,
        }
    }

    /// Release a NAR lock.
    pub async fn release_nar_lock(&self, nar_id: i64) -> WorkerResult<()> {
        match self {
            Database::D1(backend) => backend.release_nar_lock(nar_id).await,
            Database::Turso(backend) => backend.release_nar_lock(nar_id).await,
        }
    }

    /// Find which store path hashes already exist in a cache.
    pub async fn find_existing_paths(
        &self,
        cache_name: &str,
        hashes: &[String],
    ) -> WorkerResult<Vec<String>> {
        match self {
            Database::D1(backend) => backend.find_existing_paths(cache_name, hashes).await,
            Database::Turso(backend) => backend.find_existing_paths(cache_name, hashes).await,
        }
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
        match self {
            Database::D1(backend) => {
                backend
                    .update_cache(
                        name,
                        is_public,
                        priority,
                        compression,
                        retention_period,
                        upstream_cache_key_names,
                        keypair,
                    )
                    .await
            }
            Database::Turso(backend) => {
                backend
                    .update_cache(
                        name,
                        is_public,
                        priority,
                        compression,
                        retention_period,
                        upstream_cache_key_names,
                        keypair,
                    )
                    .await
            }
        }
    }

    /// Soft-delete a cache by setting deleted_at.
    ///
    /// Returns true if a cache was deleted, false if not found.
    pub async fn delete_cache(&self, name: &str) -> WorkerResult<bool> {
        match self {
            Database::D1(backend) => backend.delete_cache(name).await,
            Database::Turso(backend) => backend.delete_cache(name).await,
        }
    }
}
