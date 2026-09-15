//! Windows-only fallback for files the OS refuses to open normally (SAM,
//! SYSTEM, security hives, $MFT, $UsnJrnl, ...): parse the volume's NTFS
//! directly through a raw handle and stage the file into a private temp dir.
//!
//! Hardening vs. the original implementation:
//! - staged files land in a per-process temp dir and are unlinked as soon as
//!   the pipeline has the handle open - no SAM/SYSTEM copies left in the
//!   process working directory;
//! - the ntfs crate (C-adjacent parsing of untrusted on-disk structures) runs
//!   inside `catch_unwind`: a panic in filesystem parsing skips one artifact,
//!   it cannot destroy the whole collection;
//! - 1 MiB buffered raw reads instead of the 8 KiB default.

use crate::ntfs_driver::{cd, get, CommandInfo};
use crate::sector_reader::SectorReader;
use anyhow::{anyhow, Context, Result};
use log::debug;
use ntfs::Ntfs;
use std::fs::File;
use std::io::BufReader;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

pub fn ll_disk_access(path: &Path) -> Result<File> {
    // Split "C:\Windows\Path\$MFT[:stream]" into drive, directories, file.
    let display = path
        .to_str()
        .with_context(|| format!("raw access needs UTF-8 paths, got {}", path.display()))?;
    let mut parts = display.split('\\').filter(|p| !p.is_empty());
    let label = parts
        .next()
        .and_then(|p| p.split(':').next())
        .filter(|p| p.len() == 1)
        .with_context(|| format!("no drive label in {}", path.display()))?;
    let mut dirs: Vec<&str> = parts.collect();
    if dirs.is_empty() {
        return Err(anyhow!("no file component in {}", path.display()));
    }
    // Keep any ":stream" suffix intact: get() extracts named $DATA streams
    // (e.g. $UsnJrnl:$J) by splitting the argument itself.
    let filename = dirs.pop().unwrap();

    // Private staging dir; files are unlinked right after opening.
    let staging = std::env::temp_dir().join(format!("tamatoa-raw-{}", std::process::id()));
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;

    debug!(
        "raw-access fallback for {} via \\\\.\\{}:",
        path.display(),
        label
    );
    let opened = catch_unwind(AssertUnwindSafe(|| -> Result<PathBuf> {
        let f = File::open(format!("\\\\.\\{label}:"))
            .with_context(|| format!("opening raw volume {label} (admin privileges required?)"))?;
        let sr = SectorReader::new(f, 4096)?;
        let mut fs = BufReader::with_capacity(1024 * 1024, sr);
        let mut ntfs = Ntfs::new(&mut fs).context("parsing NTFS boot structures")?;
        ntfs.read_upcase_table(&mut fs)?;
        let current_directory = vec![ntfs.root_directory(&mut fs)?];
        let mut info = CommandInfo {
            current_directory,
            current_directory_string: String::new(),
            fs,
            ntfs: &ntfs,
        };
        for directory in &dirs {
            cd(directory, &mut info)?;
        }
        get(filename, &staging, &mut info)
    }));

    let staged = match opened {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return Err(e),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            return Err(anyhow!(
                "raw NTFS access panicked while reading {}: {msg}",
                path.display()
            ));
        }
    };

    let file =
        File::open(&staged).with_context(|| format!("opening staged {}", staged.display()))?;
    // Pipeline now holds the handle; remove the plaintext copy from disk.
    if let Err(e) = std::fs::remove_file(&staged) {
        debug!("could not unlink staged file {}: {e}", staged.display());
    }
    Ok(file)
}
