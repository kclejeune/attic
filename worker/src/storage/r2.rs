//! R2 storage backend for Cloudflare Workers.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use worker::*;

use crate::error::{WorkerError, WorkerResult};

/// R2 storage backend.
pub struct R2Backend {
    bucket: Bucket,
}

/// Reference to a file stored in R2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct R2RemoteFile {
    /// Bucket name.
    pub bucket: String,
    /// Object key.
    pub key: String,
}

/// Download result.
pub enum Download {
    /// A URL to redirect to (presigned or public).
    #[allow(dead_code)]
    Url(String),
    /// Raw bytes.
    Bytes(Bytes),
}

impl R2Backend {
    /// Create a new R2 backend with the given bucket.
    pub fn new(bucket: Bucket) -> Self {
        Self { bucket }
    }

    /// Upload a file to R2.
    pub async fn upload_file(&self, key: &str, data: Vec<u8>) -> WorkerResult<R2RemoteFile> {
        self.bucket
            .put(key, data)
            .execute()
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to upload to R2: {}", e)))?;

        Ok(R2RemoteFile {
            bucket: "cache".to_string(), // Bucket name from binding
            key: key.to_string(),
        })
    }

    /// Download a file from R2.
    pub async fn download_file(&self, key: &str) -> WorkerResult<Download> {
        let object = self
            .bucket
            .get(key)
            .execute()
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to get from R2: {}", e)))?;

        match object {
            Some(obj) => {
                let body = obj
                    .body()
                    .ok_or_else(|| WorkerError::Storage("Object has no body".to_string()))?;

                let bytes = body
                    .bytes()
                    .await
                    .map_err(|e| WorkerError::Storage(format!("Failed to read body: {}", e)))?;

                Ok(Download::Bytes(Bytes::from(bytes)))
            }
            None => Err(WorkerError::NotFound(format!("Object not found: {}", key))),
        }
    }

    /// Delete a file from R2.
    pub async fn delete_file(&self, key: &str) -> WorkerResult<()> {
        self.bucket
            .delete(key)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to delete from R2: {}", e)))?;

        Ok(())
    }

    /// Check if a file exists in R2.
    #[allow(dead_code)]
    pub async fn file_exists(&self, key: &str) -> WorkerResult<bool> {
        let object = self
            .bucket
            .head(key)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to head from R2: {}", e)))?;

        Ok(object.is_some())
    }

    /// Get file metadata without downloading.
    #[allow(dead_code)]
    pub async fn file_metadata(&self, key: &str) -> WorkerResult<Option<R2ObjectMetadata>> {
        let object = self
            .bucket
            .head(key)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to head from R2: {}", e)))?;

        Ok(object.map(|obj| R2ObjectMetadata {
            size: obj.size() as u64,
            etag: obj.etag(),
        }))
    }
}

/// Metadata about an R2 object.
#[allow(dead_code)]
pub struct R2ObjectMetadata {
    /// Size in bytes.
    pub size: u64,
    /// ETag.
    pub etag: String,
}
