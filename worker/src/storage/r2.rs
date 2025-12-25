//! R2 storage backend for Cloudflare Workers.

use serde::{Deserialize, Serialize};
use worker::*;

use crate::error::{WorkerError, WorkerResult};

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

    /// Download a file from R2 as a stream (for large files).
    /// Returns the response body as a worker Response directly.
    pub async fn download_file_stream(&self, key: &str) -> WorkerResult<worker::Response> {
        let object = self
            .bucket
            .get(key)
            .execute()
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to get from R2: {}", e)))?;

        match object {
            Some(obj) => {
                let size = obj.size();
                let body = obj
                    .body()
                    .ok_or_else(|| WorkerError::Storage("Object has no body".to_string()))?;

                // Get the readable stream directly from the body
                let stream = body
                    .stream()
                    .map_err(|e| WorkerError::Storage(format!("Failed to get stream: {:?}", e)))?;

                // Create response with streaming body
                let mut headers = worker::Headers::new();
                headers
                    .set("Content-Type", "application/x-nix-nar")
                    .map_err(|e| WorkerError::Storage(format!("Failed to set header: {:?}", e)))?;
                headers
                    .set("Content-Length", &size.to_string())
                    .map_err(|e| WorkerError::Storage(format!("Failed to set header: {:?}", e)))?;

                worker::Response::from_stream(stream)
                    .map(|r| r.with_headers(headers))
                    .map_err(|e| {
                        WorkerError::Storage(format!("Failed to create response: {:?}", e))
                    })
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

    /// Resume a multipart upload from a previous upload ID.
    ///
    /// This is used for chunked uploads where the upload state is passed between
    /// requests via a token. The `parts_uploaded` parameter indicates how many
    /// parts have already been uploaded, so the part numbering continues correctly.
    pub async fn resume_multipart_upload(
        &self,
        key: &str,
        upload_id: &str,
        parts_uploaded: u16,
    ) -> WorkerResult<R2MultipartUpload> {
        let upload = self
            .bucket
            .resume_multipart_upload(key, upload_id)
            .map_err(|e| {
                WorkerError::Storage(format!("Failed to resume multipart upload: {}", e))
            })?;

        Ok(R2MultipartUpload {
            bucket: "cache".to_string(),
            key: key.to_string(),
            upload,
            parts: Vec::new(),
            part_number: parts_uploaded + 1,
        })
    }

    /// Get file size from R2 metadata.
    pub async fn file_size(&self, key: &str) -> WorkerResult<Option<u64>> {
        let object = self
            .bucket
            .head(key)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to head from R2: {}", e)))?;

        Ok(object.map(|obj| obj.size() as u64))
    }
}

/// Serializable info about an uploaded part.
/// This can be stored in a token and used to complete the upload later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadedPartInfo {
    /// The part number (1-indexed).
    pub part_number: u16,
    /// The ETag returned by R2 for this part.
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
    /// Get the upload ID and key for resuming this upload later.
    ///
    /// Returns (upload_id, key) which can be stored and passed to
    /// `R2Backend::resume_multipart_upload` to continue the upload.
    pub async fn get_upload_info(&self) -> (String, String) {
        (self.upload.upload_id().await, self.key.clone())
    }

    /// Upload a part.
    ///
    /// Parts must be at least 5MB (except the last part). Returns info about the
    /// uploaded part that can be serialized and used to complete the upload later.
    pub async fn upload_part(&mut self, data: Vec<u8>) -> WorkerResult<UploadedPartInfo> {
        let part = self
            .upload
            .upload_part(self.part_number, data)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to upload part: {}", e)))?;

        let part_info = UploadedPartInfo {
            part_number: self.part_number,
            etag: part.etag().to_string(),
        };

        self.parts.push(part);
        self.part_number += 1;
        Ok(part_info)
    }

    /// Complete the multipart upload with explicitly provided parts.
    /// Use this when resuming an upload and you have the parts info from previous requests.
    pub async fn complete_with_parts(
        self,
        parts: Vec<UploadedPartInfo>,
    ) -> WorkerResult<R2RemoteFile> {
        // Convert stored part info back to UploadedPart objects
        let mut r2_parts: Vec<UploadedPart> = Vec::with_capacity(parts.len() + self.parts.len());

        // Add parts from previous requests (reconstructed from stored info)
        for part_info in parts {
            r2_parts.push(UploadedPart::new(part_info.part_number, part_info.etag));
        }

        // Add any parts we uploaded in this request
        r2_parts.extend(self.parts);

        // Sort by part number to ensure correct order
        r2_parts.sort_by_key(|p| p.part_number());

        self.upload
            .complete(r2_parts)
            .await
            .map_err(|e| WorkerError::Storage(format!("Failed to complete multipart: {}", e)))?;

        Ok(R2RemoteFile {
            bucket: self.bucket,
            key: self.key,
        })
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
