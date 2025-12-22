//! R2 storage backend for Cloudflare Workers.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use worker::*;

use crate::error::{WorkerError, WorkerResult};

/// Minimum part size for R2 multipart upload (5MB).
/// R2 requires parts to be at least 5MB (except the last part).
#[allow(dead_code)]
pub const MIN_PART_SIZE: usize = 5 * 1024 * 1024;

/// Target part size for optimal throughput (8MB).
/// Using 8MB gives good balance between memory usage and upload efficiency.
pub const TARGET_PART_SIZE: usize = 8 * 1024 * 1024;

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

    /// Start a multipart upload.
    ///
    /// This allows uploading large files in parts, which is essential for streaming
    /// compression where the final size is not known upfront. Each part (except the
    /// last) must be at least 5MB.
    pub async fn create_multipart_upload(&self, key: &str) -> WorkerResult<R2MultipartUpload> {
        let upload = self
            .bucket
            .create_multipart_upload(key)
            .execute()
            .await
            .map_err(|e| {
                WorkerError::Storage(format!("Failed to create multipart upload: {}", e))
            })?;

        Ok(R2MultipartUpload {
            bucket: "cache".to_string(),
            key: key.to_string(),
            upload,
            parts: Vec::new(),
            part_number: 1,
        })
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

/// Active multipart upload handle.
///
/// Use this to upload large files in parts. Each part (except the last) must be
/// at least 5MB. After uploading all parts, call `complete()` to finalize the
/// upload, or `abort()` to cancel and clean up.
pub struct R2MultipartUpload {
    bucket: String,
    key: String,
    upload: MultipartUpload,
    parts: Vec<UploadedPart>,
    part_number: u16,
}

impl R2MultipartUpload {
    /// Upload a part.
    ///
    /// Parts must be at least 5MB (except the last part). Returns the part number
    /// that was uploaded. Parts are tracked internally and will be used when
    /// completing the upload.
    pub async fn upload_part(&mut self, data: Vec<u8>) -> WorkerResult<u16> {
        let part = self
            .upload
            .upload_part(self.part_number, data)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to upload part: {}", e)))?;

        self.parts.push(part);
        let uploaded_part = self.part_number;
        self.part_number += 1;
        Ok(uploaded_part)
    }

    /// Get the number of parts uploaded so far.
    #[allow(dead_code)]
    pub fn parts_count(&self) -> usize {
        self.parts.len()
    }

    /// Complete the multipart upload.
    ///
    /// This finalizes the upload and makes the object available. All uploaded parts
    /// are combined into the final object. After calling this, the upload handle
    /// is consumed and cannot be used again.
    pub async fn complete(self) -> WorkerResult<R2RemoteFile> {
        self.upload
            .complete(self.parts)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to complete multipart: {}", e)))?;

        Ok(R2RemoteFile {
            bucket: self.bucket,
            key: self.key,
        })
    }

    /// Abort the multipart upload.
    ///
    /// This cancels the upload and cleans up any uploaded parts. Use this for
    /// error recovery to avoid leaving orphaned parts in R2.
    pub async fn abort(self) -> WorkerResult<()> {
        self.upload
            .abort()
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to abort multipart: {}", e)))?;
        Ok(())
    }
}
