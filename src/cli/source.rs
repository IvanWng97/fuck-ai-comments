use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};

// Bounds parser allocation so a hostile source cannot exhaust a required CI gate.
pub(super) const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn read_regular(disk_path: &Path, source_path: &Path) -> Result<Vec<u8>> {
    let path_metadata = fs::symlink_metadata(disk_path)
        .with_context(|| format!("could not inspect {}", source_path.display()))?;
    if !path_metadata.file_type().is_file() {
        bail!(
            "supported path {} is not a regular file",
            source_path.display()
        );
    }

    let file = File::open(disk_path)
        .with_context(|| format!("could not open {}", source_path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("could not inspect open file {}", source_path.display()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "supported path {} is not a regular file",
            source_path.display()
        );
    }
    reject_oversized(source_path, metadata.len())?;

    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("could not read {}", source_path.display()))?;
    reject_oversized(
        source_path,
        u64::try_from(bytes.len()).context("source size exceeded address space")?,
    )?;
    Ok(bytes)
}

pub(super) fn utf8<'source>(source_path: &Path, bytes: &'source [u8]) -> Result<&'source str> {
    std::str::from_utf8(bytes)
        .with_context(|| format!("{} is not valid UTF-8", source_path.display()))
}

fn reject_oversized(source_path: &Path, size: u64) -> Result<()> {
    if size > MAX_SOURCE_BYTES {
        bail!(
            "{} is {size} bytes; supported source limit is {MAX_SOURCE_BYTES} bytes",
            source_path.display()
        );
    }
    Ok(())
}
