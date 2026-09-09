use std::fs::{File, OpenOptions};
use std::path::Path;

use anyhow::{Context, Result};
use memmap2::MmapMut;

/// A pre-allocated sparse file on disk.
/// On Windows, uses SetEndOfFile to reserve the space without writing zeros.
/// On Linux/macOS, uses ftruncate which creates a sparse file natively.
pub struct SparseFile {
    file: File,
}

impl SparseFile {
    /// Create (or truncate) a file at `path` with exactly `size` bytes reserved.
    pub fn create(path: &str, size: u64) -> Result<Self> {
        // Create parent directories if needed
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {:?}", parent))?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("Failed to open file: {}", path))?;

        // Pre-allocate the file size
        file.set_len(size)?;

        Ok(Self { file })
    }

    /// Convert to a memory-mapped writable file
    pub fn into_mmap(self) -> Result<MmapFile> {
        let mmap = unsafe { MmapMut::map_mut(&self.file) }
            .context("Failed to create memory map")?;

        Ok(MmapFile { _file: self.file, mmap })
    }
}

/// An open memory-mapped file supporting concurrent zero-copy writes
pub struct MmapFile {
    _file: File, // Keep the file handle alive
    mmap: MmapMut,
}

impl MmapFile {
    /// Write `data` at byte `offset` within the mapped file.
    ///
    /// This is the zero-copy path: each segment worker calls this directly
    /// with the network bytes — no intermediate buffers required.
    pub fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<()> {
        let offset = offset as usize;
        let end = offset + data.len();

        if end > self.mmap.len() {
            anyhow::bail!(
                "Write out of bounds: offset={} len={} mmap_size={}",
                offset,
                data.len(),
                self.mmap.len()
            );
        }

        self.mmap[offset..end].copy_from_slice(data);
        Ok(())
    }

    /// Flush all dirty pages to disk
    pub fn flush(&self) -> Result<()> {
        self.mmap.flush().context("Failed to flush mmap to disk")
    }
}
