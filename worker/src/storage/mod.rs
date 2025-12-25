//! Storage backends for the Attic Worker.

mod r2;

pub use r2::{R2Backend, UploadedPartInfo, TARGET_PART_SIZE};
