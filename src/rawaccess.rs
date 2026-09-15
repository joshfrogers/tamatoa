//! Windows-only fallback for files the OS refuses to open normally (SAM,
//! SYSTEM, security hives, $MFT, $UsnJrnl, ...): parse the volume's NTFS
//! directly through a raw handle and stage the file into a private temp dir.
//!
//! Hardening vs. the original implementation:
//! - one raw volume handle + parsed NTFS metadata per drive, cached across
//!   artifacts (the old code re-opened the volume, re-parsed the boot
//!   structures, and re-read the multi-MB upcase table for *every* file);
//! - staged files land in a per-artifact private temp dir and are unlinked as
//!   soon as the pipeline has the handle open - no SAM/SYSTEM copies left in
//!   the process working directory, and same-named files from different
//!   directories (two users' NTUSER.DAT) cannot collide;
//! - the ntfs crate (C-adjacent parsing of untrusted on-disk structures) runs
//!   inside `catch_unwind`: a panic skips one artifact and evicts the cached
//!   volume; it cannot destroy the whole collection;
//! - 1 MiB buffered raw reads instead of the 8 KiB default.

use crate::ntfs_driver::{cd, get, CommandInfo};
use crate::sector_reader::SectorReader;
use anyhow::{anyhow, Context, Result};
use log::debug;
use ntfs::Ntfs;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Parsed view of one raw-opened volume, cached per thread (collection runs
/// single-threaded; thread_local keeps it lock-free).
struct Volume {
    fs: BufReader<SectorReader<File>>,
    ntfs: Ntfs,
}

thread_local! {
    static VOLUMES: RefCell<HashMap<char, Volume>> = RefCell::new(HashMap::new());
}

fn open_volume(label: char) -> Result<Volume> {
    debug!("opening raw volume \\\\.\\{label}:");
    let f = File::open(format!("\\\\.\\{label}:"))
        .with_context(|| format!("opening raw volume {label} (admin privileges required?)"))?;
    let sr = SectorReader::new(f, 4096)?;
    let mut fs = BufReader::with_capacity(1024 * 1024, sr);
    let mut ntfs = Ntfs::new(&mut fs).context("parsing NTFS boot structures")?;
    ntfs.read_upcase_table(&mut fs)?;
    Ok(Volume { fs, ntfs })
}

/// Run `f` against the cached NTFS view of `label`. The cached entry is
/// removed for the duration of the call and on any error/panic, so a volume
/// that panicked mid-parse is re-opened from scratch for the next artifact.
fn with_volume<R>(label: char, f: impl FnOnce(&mut Volume) -> Result<R>) -> Result<R> {
    let cached = VOLUMES.with(|c| c.borrow_mut().remove(&label));
    let mut vol = match cached {
        Some(v) => v,
        None => open_volume(label)?,
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut vol)));
    VOLUMES.with(|c| {
        if let Ok(Ok(_)) = outcome {
            c.borrow_mut().insert(label, vol);
        }
    });
    match outcome {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            Err(anyhow!(
                "raw NTFS access panicked while reading volume {label}: {msg}"
            ))
        }
    }
}

/// Split "C:\Windows\Path\$MFT[:stream]" into drive letter, directory
/// components, and the final file component (stream suffix preserved -
/// `get()` extracts named $DATA streams by splitting it itself).
fn split_windows_path(display: &str) -> Result<(char, Vec<&str>, &str)> {
    let mut parts = display.split('\\').filter(|p| !p.is_empty());
    let label = parts
        .next()
        .and_then(|p| p.split(':').next())
        .filter(|p| p.len() == 1)
        .and_then(|p| p.chars().next())
        .ok_or_else(|| anyhow!("no drive label in path {display:?}"))?
        .to_ascii_uppercase();
    let mut dirs: Vec<&str> = parts.collect();
    let filename = dirs
        .pop()
        .ok_or_else(|| anyhow!("no file component in path {display:?}"))?;
    Ok((label, dirs, filename))
}

/// Unique staging directory per staged artifact: the same base filename from
/// two directories (two users' NTUSER.DAT) must not collide in one run.
fn staging_dir(filename: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tamatoa-raw-{}-{seq}-{filename}",
        std::process::id()
    ))
}

static SEQ: AtomicU64 = AtomicU64::new(0);

pub fn ll_disk_access(path: &Path) -> Result<File> {
    // Raw NTFS parsing speaks UTF-8/UTF-16LE; non-Unicode paths are refused
    // rather than lossily converted (lossy = wrong file).
    let display = path
        .to_str()
        .with_context(|| format!("raw access needs UTF-8 paths, got {}", path.display()))?;
    let (label, dirs, filename) = split_windows_path(display)?;

    let staging = staging_dir(filename);
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating staging dir {}", staging.display()))?;
    debug!(
        "raw-access fallback for {} via \\\\.\\{label}:",
        path.display()
    );

    let extracted = with_volume(label, |vol| {
        let root = vol.ntfs.root_directory(&mut vol.fs)?;
        let mut info = CommandInfo {
            current_directory: vec![root],
            current_directory_string: String::new(),
            fs: &mut vol.fs,
            ntfs: &vol.ntfs,
        };
        for directory in &dirs {
            cd(directory, &mut info)?;
        }
        get(filename, &staging, &mut info)
    });

    let staged = match extracted {
        Ok(p) => p,
        Err(e) => {
            std::fs::remove_dir(&staging).ok();
            return Err(e).with_context(|| format!("raw access for {}", path.display()));
        }
    };

    let file =
        File::open(&staged).with_context(|| format!("opening staged {}", staged.display()))?;
    // Pipeline now holds the handle; remove the plaintext copy from disk.
    if let Err(e) = std::fs::remove_file(&staged) {
        debug!("could not unlink staged file {}: {e}", staged.display());
    }
    std::fs::remove_dir(&staging).ok();
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::split_windows_path;

    #[test]
    fn splits_drive_dirs_and_stream_suffix() {
        let (label, dirs, file) = split_windows_path(r"C:\Windows\System32\config\SAM").unwrap();
        assert_eq!(label, 'C');
        assert_eq!(dirs, ["Windows", "System32", "config"]);
        assert_eq!(file, "SAM");

        // Named streams stay attached to the file component.
        let (label, dirs, file) = split_windows_path(r"c:\$Extend\$UsnJrnl:$J").unwrap();
        assert_eq!(label, 'C', "drive letter normalized");
        assert_eq!(dirs, ["$Extend"]);
        assert_eq!(file, "$UsnJrnl:$J");

        assert!(split_windows_path("relative\\path").is_err());
        assert!(split_windows_path("C:\\").is_err());
    }
}
