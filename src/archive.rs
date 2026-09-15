//! Single-pass archive pipeline: every artifact is opened once, hashed while
//! being copied into the ZIP, and recorded in a JSON manifest that is embedded
//! in the archive and written beside it.
//!
//! Guarantees:
//! - entry names are host-qualified relative paths (`host/var/log/syslog`); no
//!   absolute paths, drive letters or `..` components ever reach the archive
//!   (zip-slip hardening for whoever extracts the evidence);
//! - an entry is created only after the source was successfully opened, and a
//!   failed copy aborts the entry - the archive never contains silent
//!   zero-byte placeholders;
//! - every failure (symlink, special file, permission, read error, size
//!   budget) is recorded in the manifest with a reason;
//! - the archive's own SHA-256 is computed after completion and reported
//!   outside the archive (in the sidecar manifest), where it can attest to it.

use crate::version;
use anyhow::{anyhow, Context, Result};
use log::{debug, error, warn};
use serde_json::json;
use sha2::{Digest, Sha256};

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};
/// Outcome variant shorthand used throughout this module.
use Outcome::Failed;

/// 256 KiB streaming buffer: large enough to keep syscall counts sane on bulk
/// copies, small enough to stay in L2 on any endpoint.
pub const COPY_BUF_BYTES: usize = 256 * 1024;

/// Extension list of artifacts that are already compressed (or internally
/// chunk-compressed); deflating them again burns CPU for ~0 bytes saved.
const ALREADY_COMPRESSED_EXTS: &[&str] = &[
    "7z", "avi", "bz2", "cab", "gz", "jpg", "jpeg", "lzma", "mkv", "mov", "mp4", "pf", "png",
    "prefetch", "tgz", "wim", "xz", "zip", "zst",
];

#[derive(Debug, Clone)]
pub struct ArchiveConfig {
    pub host: String,
    pub zip_level: u32,
    /// Hard cap on a single artifact's byte size (streaming guard against
    /// infinite/absurd sources). 0 disables.
    pub max_file_bytes: u64,
    /// Hard cap on total collected payload bytes. 0 disables.
    pub max_total_bytes: u64,
    /// Allow truncating a pre-existing archive file (resolve stage already
    /// refuses collisions unless force was requested).
    pub overwrite: bool,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            host: "unknownhost".into(),
            zip_level: 6,
            max_file_bytes: 128 << 30,
            max_total_bytes: 1 << 40,
            overwrite: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Collected,
    /// Source does not exist. Optional artifacts being absent is the normal
    /// case (service accounts without dotfiles); it does NOT mark the run
    /// partial. Real errors (permission, IO, budget) are `Failed`.
    NotFound(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct EntryRecord {
    pub source: PathBuf,
    pub outcome: Outcome,
    pub entry_name: Option<String>,
    pub sha256: Option<String>,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct CollectionStats {
    pub collected: usize,
    pub missing: usize,
    pub failed: usize,
    pub bytes_archived: u64,
}

impl CollectionStats {
    pub fn is_partial(&self) -> bool {
        self.failed > 0
    }
}

/// Convert a collected filesystem path into a safe, host-qualified relative
/// zip entry name. Returns Err for paths containing parent-directory components
/// (which a hostile volume could otherwise use for traversal on extraction).
pub fn entry_name(host: &str, path: &Path) -> Result<String> {
    let mut parts: Vec<String> = vec![host.to_string()];
    for comp in path.components() {
        match comp {
            Component::Normal(c) => {
                let s = c.to_string_lossy();
                if s == ".." || s.contains('\0') {
                    return Err(anyhow!("path component escapes collection root: {s}"));
                }
                parts.push(
                    s.chars()
                        .map(|ch| match ch {
                            '\\' => '/',
                            // ':' is illegal in names on the extraction side
                            // (drive/stream syntax); underscore it.
                            ':' => '_',
                            other => other,
                        })
                        .filter(|ch| !ch.is_control())
                        .collect(),
                );
            }
            Component::Prefix(p) => {
                // Windows drive/UNC prefix: "C:\..." -> component "c".
                let prefix = p.as_os_str().to_string_lossy().to_lowercase();
                let cleaned: String = prefix.chars().filter(|c| c.is_alphanumeric()).collect();
                parts.push(if cleaned.is_empty() {
                    "root".to_string()
                } else {
                    cleaned
                });
            }
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow!("path contains '..': {}", path.display()));
            }
        }
    }
    if parts.len() < 2 {
        return Err(anyhow!(
            "path has no collectable components: {}",
            path.display()
        ));
    }
    Ok(parts.join("/"))
}

fn should_deflate(path: &Path, level: u32) -> bool {
    if level == 0 {
        return false;
    }
    !path
        .extension()
        .map(|e| {
            let e = e.to_string_lossy().to_lowercase();
            ALREADY_COMPRESSED_EXTS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

fn file_options_for(path: &Path, cfg: &ArchiveConfig) -> SimpleFileOptions {
    let (method, level) = if should_deflate(path, cfg.zip_level) {
        (CompressionMethod::Deflated, Some(cfg.zip_level as i64))
    } else {
        (CompressionMethod::Stored, None)
    };
    SimpleFileOptions::default()
        .compression_method(method)
        .compression_level(level)
        .last_modified_time(zip::DateTime::default())
        .large_file(true)
        .unix_permissions(0o644)
}

/// Open a collected artifact: never follows symlinks (already rejected by
/// metadata preflight, but the flag closes the TOCTOU window), and on Windows
/// falls back to raw volume reads for files locked by the OS.
fn open_artifact(path: &Path) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    match opts.open(path) {
        Ok(f) => Ok(f),
        #[cfg(windows)]
        Err(e) => crate::rawaccess::ll_disk_access(path)
            .with_context(|| format!("open failed ({e}), raw volume fallback also failed")),
        #[cfg(not(windows))]
        Err(e) => Err(e).with_context(|| format!("opening {}", path.display())),
    }
}

/// Preflight classification before anything is opened.
fn classify(path: &Path) -> Option<Outcome> {
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) => {
            let msg = format!("stat failed: {e}");
            return Some(if e.kind() == std::io::ErrorKind::NotFound {
                Outcome::NotFound(msg)
            } else {
                Outcome::Failed(msg)
            });
        }
    };
    let ft = md.file_type();
    if ft.is_symlink() {
        let target = std::fs::read_link(path)
            .map(|t| t.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "?".into());
        return Some(Outcome::Failed(format!(
            "symlink to {target} (not followed)"
        )));
    }
    if ft.is_dir() {
        return Some(Outcome::Failed(
            "directory (collectors must expand directories)".into(),
        ));
    }
    if !ft.is_file() {
        return Some(Outcome::Failed("special file (fifo/socket/device)".into()));
    }
    None
}

/// Process exactly one artifact: classify, name, open, stream into the zip,
/// record outcome. Split out so `collect` can run it inside `catch_unwind`.
fn collect_one(
    path: &Path,
    zip: &mut ZipWriter<BufWriter<&mut File>>,
    cfg: &ArchiveConfig,
    stats: &mut CollectionStats,
    records: &mut Vec<EntryRecord>,
    entry_names: &mut HashSet<String>,
    copy_buf: &mut [u8],
) {
    let mut record =
        |outcome: Outcome, entry_name: Option<String>, sha256: Option<String>, size: u64| {
            records.push(EntryRecord {
                source: path.to_path_buf(),
                outcome,
                entry_name,
                sha256,
                size,
            });
        };

    if let Some(outcome) = classify(path) {
        let missing = matches!(outcome, Outcome::NotFound(_));
        if missing {
            debug!("artifact absent {}: {outcome:?}", path.display());
            stats.missing += 1;
        } else {
            warn!("{}: skipped: {outcome:?}", path.display());
            stats.failed += 1;
        }
        record(outcome, None, None, 0);
        return;
    }

    let entry = match entry_name(&cfg.host, path) {
        Ok(e) => e,
        Err(e) => {
            stats.failed += 1;
            record(Failed(e.to_string()), None, None, 0);
            return;
        }
    };
    if !entry_names.insert(entry.clone()) {
        stats.failed += 1;
        record(
            Failed(format!("duplicate zip entry name: {entry}")),
            None,
            None,
            0,
        );
        return;
    }

    // The entry starts only once the bytes are definitely coming: no
    // placeholder entries for files that cannot be opened.
    let mut src = match open_artifact(path) {
        Ok(f) => f,
        Err(e) => {
            entry_names.remove(&entry);
            // TOCTOU race after classify, or unreadable: distinguish a
            // vanished file from a real I/O error.
            let gone = e
                .chain()
                .find_map(|c| c.downcast_ref::<std::io::Error>())
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
            let outcome = if gone {
                Outcome::NotFound(e.to_string())
            } else {
                Failed(e.to_string())
            };
            if gone {
                debug!("{} vanished before open", path.display());
                stats.missing += 1;
            } else {
                warn!("{}: skipped: {e}", path.display());
                stats.failed += 1;
            }
            record(outcome, None, None, 0);
            return;
        }
    };

    if let Err(e) = zip.start_file(&entry, file_options_for(path, cfg)) {
        stats.failed += 1;
        record(Failed(format!("zip start: {e}")), None, None, 0);
        return;
    }

    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut failure: Option<String> = None;
    loop {
        if cfg.max_file_bytes > 0 && size > cfg.max_file_bytes {
            failure = Some(format!(
                "exceeded per-file budget of {} bytes",
                cfg.max_file_bytes
            ));
            break;
        }
        if cfg.max_total_bytes > 0 && stats.bytes_archived + size > cfg.max_total_bytes {
            failure = Some(format!(
                "exceeded total collection budget of {} bytes",
                cfg.max_total_bytes
            ));
            break;
        }
        match src.read(copy_buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&copy_buf[..n]);
                if let Err(e) = zip.write_all(&copy_buf[..n]) {
                    failure = Some(format!("archive write failed: {e}"));
                    break;
                }
                size += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                failure = Some(format!("read failed: {e}"));
                break;
            }
        }
    }

    match failure {
        None => {
            stats.collected += 1;
            stats.bytes_archived += size;
            record(
                Outcome::Collected,
                Some(entry),
                Some(hex_digest(&hasher)),
                size,
            );
        }
        Some(reason) => {
            // Drop the incomplete entry entirely: a truncated file that
            // looks complete is worse forensic evidence than none.
            if let Err(e) = zip.abort_file() {
                warn!("aborting entry {entry}: {e}");
            }
            entry_names.remove(&entry);
            stats.failed += 1;
            record(Failed(reason), None, None, 0);
        }
    }
}

/// Collect `paths` into a new zip archive. Returns stats plus the archive's
/// hex SHA-256. Per-artifact failures are recorded, never fatal; structural
/// failures (cannot create output) return Err.
pub fn collect(
    paths: &[PathBuf],
    archive_path: &Path,
    cfg: &ArchiveConfig,
) -> Result<CollectionStats> {
    use Outcome::Failed;
    // Deterministic order + one entry per source path (default lists and
    // user-supplied globs overlap routinely).
    let mut sorted: Vec<&PathBuf> = paths.iter().collect();
    sorted.sort();
    sorted.dedup();

    let mut file = {
        let mut o = OpenOptions::new();
        // read+write: the completion hash re-reads the finished archive.
        o.read(true).write(true);
        if cfg.overwrite {
            o.create(true).truncate(true);
        } else {
            o.create_new(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(archive_path)
            .with_context(|| format!("creating archive {}", archive_path.display()))?
    };

    // 1 MiB write buffer under the zip writer: the zip encoder emits many
    // small writes; without this the OS sees one syscall each.
    let buf = BufWriter::with_capacity(1024 * 1024, &mut file);
    let mut zip = ZipWriter::new(buf);

    let mut records: Vec<EntryRecord> = Vec::with_capacity(sorted.len());
    let mut stats = CollectionStats::default();
    let mut entry_names: HashSet<String> = HashSet::new();
    let mut copy_buf = vec![0u8; COPY_BUF_BYTES];

    for path in sorted {
        // One artifact must never take down the run: zip-crate bugs, exotic
        // paths, or panics in platform fallbacks become a recorded failure,
        // the archive still finishes.
        let guarded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            collect_one(
                path,
                &mut zip,
                cfg,
                &mut stats,
                &mut records,
                &mut entry_names,
                &mut copy_buf,
            )
        }));
        if let Err(payload) = guarded {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".to_string());
            error!("panic while collecting {}: {msg}", path.display());
            // If the panic happened after start_file, drop the partial entry
            // (an error here just means no entry was open: safe to ignore).
            if zip.abort_file().is_ok() {
                if let Some(name) = entry_name(&cfg.host, path).ok() {
                    entry_names.remove(&name);
                }
            }
            stats.failed += 1;
            records.push(EntryRecord {
                source: path.clone(),
                outcome: Failed(format!("panicked while collecting: {msg}")),
                entry_name: None,
                sha256: None,
                size: 0,
            });
        }
    }

    // Manifest: written into the archive (evidence self-description) and again
    // beside it (orchestrator-readable without unzipping). The in-archive copy
    // carries no wall-clock timestamps so identical sources give byte-identical
    // archives; the sidecar adds the timing and the archive's own digest.
    let entries_json: Vec<_> = records
        .iter()
        .map(|r| {
            json!({
                "source": r.source.to_string_lossy(),
                "entry": r.entry_name,
                "sha256": r.sha256,
                "size": r.size,
                "status": match &r.outcome {
                    Outcome::Collected => json!("collected"),
                    Outcome::NotFound(_) => json!("missing"),
                    Outcome::Failed(reason) => json!({"failed": reason}),
                }
            })
        })
        .collect();

    let manifest_body = json!({
        "tool": { "name": "tamatoa", "version": version::VERSION_STRING.as_str(), "host": cfg.host },
        "summary": {
            "attempted": records.len(),
            "collected": stats.collected,
            "missing": stats.missing,
            "failed": stats.failed,
            "bytes_archived": stats.bytes_archived,
        },
        "entries": entries_json,
    });

    let manifest_name = "tamatoa_manifest.json";
    zip.start_file(manifest_name, SimpleFileOptions::default())
        .context("manifest entry")
        .map_err(|e| anyhow!("{e}"))?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest_body)?;
    zip.write_all(&manifest_bytes)?;

    let mut bufw = zip.finish()?;
    bufw.flush()?;
    drop(bufw);
    // Hash the completed archive from disk: the zip encoder back-patches local
    // headers by seeking, so a write-time stream hash would not describe the
    // final bytes. One buffered sequential re-read is the honest cost.
    file.seek(SeekFrom::Start(0))?;
    let mut arch_hasher = Sha256::new();
    let mut hash_buf = vec![0u8; 1024 * 1024];
    let archive_size = file.metadata()?.len();
    let mut remaining = archive_size;
    while remaining > 0 {
        let want = std::cmp::min(remaining, hash_buf.len() as u64) as usize;
        let n = file.read(&mut hash_buf[..want])?;
        if n == 0 {
            break;
        }
        arch_hasher.update(&hash_buf[..n]);
        remaining -= n as u64;
    }
    let archive_sha = to_hex(arch_hasher.finalize());

    // Sidecar manifest beside the archive (adds timing + archive digest).
    let sidecar_path = sidecar_name(archive_path);
    let sidecar_body = json!({
        "collected_at_unix": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "archive": {
            "path": archive_path.to_string_lossy(),
            "sha256": archive_sha,
            "size": archive_size,
        },
        "manifest": manifest_body,
    });
    let mut sidecar = {
        let mut o = OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        o.open(&sidecar_path)
            .with_context(|| format!("creating sidecar manifest {}", sidecar_path.display()))?
    };
    sidecar.write_all(&serde_json::to_vec_pretty(&sidecar_body)?)?;
    sidecar.flush()?;

    Ok(stats)
}

pub fn sidecar_name(archive_path: &Path) -> PathBuf {
    let mut name = archive_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tamatoa.zip".to_string());
    name.push_str(".manifest.json");
    archive_path.with_file_name(name)
}

fn to_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_digest(hasher: &Sha256) -> String {
    to_hex(hasher.clone().finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tamatoa-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn entry_names_are_host_relative_and_safe() {
        assert_eq!(
            entry_name("SRV1", Path::new("/var/log/syslog")).unwrap(),
            "SRV1/var/log/syslog"
        );
        assert_eq!(
            entry_name("SRV1", Path::new("relative/dir/file.txt")).unwrap(),
            "SRV1/relative/dir/file.txt"
        );
        // ':' would collide with drive/stream syntax on Windows extraction.
        assert_eq!(
            entry_name("SRV1", Path::new("/we:ird/name")).unwrap(),
            "SRV1/we_ird/name"
        );
        // Parent traversal is refused, not silently normalized.
        assert!(entry_name("SRV1", Path::new("/var/../../etc/passwd")).is_err());
        assert!(entry_name("SRV1", Path::new("/")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn entry_names_flatten_windows_drives() {
        assert_eq!(
            entry_name(
                "WKS1",
                Path::new("C:\\Windows\\System32\\winevt\\Logs\\x.evtx")
            )
            .unwrap(),
            "WKS1/c/Windows/System32/winevt/Logs/x.evtx"
        );
    }

    #[test]
    fn compressed_extensions_are_stored() {
        // .pf (Windows prefetch) is internally compressed: stored, not deflated
        assert!(!should_deflate(Path::new("x/AppEvent.LOG1.PF"), 6));
        assert!(!should_deflate(Path::new("x/AppEvent.log1.prefetch"), 6));
        assert!(!should_deflate(Path::new("x/archive.zip"), 6));
        assert!(should_deflate(Path::new("x/plain.log"), 6));
        assert!(!should_deflate(Path::new("x/plain.log"), 0)); // -zl 0 => all stored
    }

    #[test]
    fn collect_happy_path_hashes_and_names() {
        let dir = temp_fixture("happy");
        std::fs::write(dir.join("a.txt"), b"alpha\n").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/b.bin"), [7u8; 5000]).unwrap();
        let archive = dir.join("out.zip");

        let stats = collect(
            &[dir.join("a.txt"), dir.join("sub/b.bin")],
            &archive,
            &ArchiveConfig {
                host: "TESTHOST".into(),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(stats.collected, 2);
        assert_eq!(stats.failed, 0);

        let mut z = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let names: Vec<String> = z.file_names().map(|s| s.to_string()).collect();
        assert!(
            names
                .iter()
                .all(|n| !n.starts_with('/') && !n.contains(':')),
            "entry names must be host-relative: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.ends_with("/a.txt") && n.starts_with("TESTHOST/")),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.ends_with("tamatoa_manifest.json")));

        // The archived bytes must match the source exactly...
        let a_name = names
            .iter()
            .find(|n| n.ends_with("/a.txt"))
            .unwrap()
            .clone();
        let mut got = String::new();
        {
            let mut entry = z.by_name(&a_name).unwrap();
            std::io::Read::read_to_string(&mut entry, &mut got).unwrap();
        }
        assert_eq!(got, "alpha\n");

        // ...and the manifest's attestation must be the true SHA-256.
        let mut h = Sha256::new();
        h.update(b"alpha\n");
        let want = to_hex(h.finalize());
        let man: serde_json::Value = {
            let raw: Vec<u8> = z
                .by_name("tamatoa_manifest.json")
                .unwrap()
                .bytes()
                .collect::<std::io::Result<Vec<_>>>()
                .unwrap();
            serde_json::from_slice(&raw).unwrap()
        };
        let a_entry = man["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["entry"].as_str().is_some_and(|s| s.ends_with("/a.txt")))
            .unwrap();
        assert_eq!(a_entry["sha256"], want);
        assert_eq!(man["summary"]["collected"], 2);

        // Sidecar attests the archive itself.
        let side = std::fs::read(sidecar_name(&archive)).unwrap();
        let side: serde_json::Value = serde_json::from_slice(&side).unwrap();
        let mut af = File::open(&archive).unwrap();
        let mut abuf = Vec::new();
        std::io::Read::read_to_end(&mut af, &mut abuf).unwrap();
        let mut ah = Sha256::new();
        ah.update(&abuf);
        assert_eq!(side["archive"]["sha256"], to_hex(ah.finalize()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn absent_sources_are_missing_not_failed() {
        let dir = temp_fixture("missing");
        std::fs::write(dir.join("present.txt"), b"x").unwrap();
        std::fs::create_dir(dir.join("adir")).unwrap();
        // Absent file (parent exists): normal for optional artifacts.
        let absent = dir.join("absent.txt");
        // Directory: a real failure (collector contract violation).
        let stats = collect(
            &[dir.join("present.txt"), absent, dir.join("adir")],
            &dir.join("out.zip"),
            &ArchiveConfig {
                host: "T".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.collected, 1);
        assert_eq!(stats.missing, 1, "absent source classified missing");
        assert_eq!(stats.failed, 1, "directory is a real failure");
        assert!(stats.is_partial(), "failed>0 marks the run partial");

        // The manifest records the status taxonomy verbatim.
        let mut z = zip::ZipArchive::new(File::open(dir.join("out.zip")).unwrap()).unwrap();
        let m: serde_json::Value =
            serde_json::from_reader(z.by_name("tamatoa_manifest.json").unwrap()).unwrap();
        let statuses: Vec<&str> = m["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["status"].as_str().unwrap_or("failed"))
            .collect();
        assert!(statuses.contains(&"missing"), "{statuses:?}");
        assert!(statuses.contains(&"collected"), "{statuses:?}");
        assert_eq!(m["summary"]["missing"], serde_json::json!(1));
    }

    #[test]
    fn only_missing_sources_still_exits_complete() {
        let dir = temp_fixture("onlymissing");
        std::fs::write(dir.join("p.txt"), b"x").unwrap();
        let stats = collect(
            &[dir.join("p.txt"), dir.join("gone.txt")],
            &dir.join("out.zip"),
            &ArchiveConfig {
                host: "T".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.missing, 1);
        assert!(!stats.is_partial(), "absence alone is not partial");
    }

    #[test]
    fn deterministic_output_is_reproducible() {
        let dir = temp_fixture("determinism");
        std::fs::write(dir.join("f1"), b"one\n").unwrap();
        std::fs::write(dir.join("f2"), b"two\n").unwrap();
        let cfg = ArchiveConfig {
            host: "H".into(),
            zip_level: 6,
            ..Default::default()
        };
        let paths = vec![dir.join("f2"), dir.join("f1")]; // shuffled input
        collect(&paths, &dir.join("r1.zip"), &cfg).unwrap();
        collect(&paths, &dir.join("r2.zip"), &cfg).unwrap();
        assert_eq!(
            std::fs::read(dir.join("r1.zip")).unwrap(),
            std::fs::read(dir.join("r2.zip")).unwrap(),
            "same sources must produce byte-identical archives"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_fifos_are_skipped_not_followed() {
        use std::ffi::CString;
        use std::os::unix::fs as ufs;
        let dir = temp_fixture("special");
        std::fs::write(dir.join("real.txt"), b"secret target\n").unwrap();
        ufs::symlink(dir.join("real.txt"), dir.join("link.txt")).unwrap();
        ufs::symlink("/nonexistent-tamatoa-test", dir.join("broken.txt")).unwrap();
        let fifo = dir.join("afifo");
        let fifo_c = CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o644) }, 0);
        let archive = dir.join("out.zip");

        let stats = collect(
            &[
                dir.join("link.txt"),
                dir.join("broken.txt"),
                fifo.clone(),
                dir.join("real.txt"),
            ],
            &archive,
            &ArchiveConfig {
                host: "H".into(),
                ..Default::default()
            },
        )
        .unwrap();

        // FIFOs must be skipped, never opened (opening one blocks forever).
        assert_eq!(stats.collected, 1, "only the regular file is collected");
        assert_eq!(stats.failed, 3);
        let z = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        let data_entries: Vec<&str> = z
            .file_names()
            .filter(|n| !n.ends_with("tamatoa_manifest.json"))
            .collect();
        assert_eq!(data_entries.len(), 1, "{data_entries:?}");
        assert!(data_entries[0].ends_with("/real.txt"), "{data_entries:?}");
    }

    #[test]
    fn per_file_budget_rejects_oversize_artifact() {
        let dir = temp_fixture("budget");
        std::fs::write(dir.join("big.bin"), vec![9u8; 4096]).unwrap();
        std::fs::write(dir.join("small.txt"), b"fits\n").unwrap();
        let stats = collect(
            &[dir.join("big.bin"), dir.join("small.txt")],
            &dir.join("out.zip"),
            &ArchiveConfig {
                host: "H".into(),
                max_file_bytes: 1024,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.collected, 1);
        assert_eq!(stats.failed, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn duplicate_and_overlap_paths_yield_one_entry() {
        let dir = temp_fixture("dupes");
        std::fs::write(dir.join("only.txt"), b"x").unwrap();
        let p = dir.join("only.txt");
        let stats = collect(
            &[p.clone(), p.clone()],
            &dir.join("out.zip"),
            &ArchiveConfig {
                host: "H".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(stats.collected, 1);
        assert_eq!(stats.failed, 0, "exact duplicates dedupe, not fail");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn existing_archive_is_not_clobbered_without_overwrite() {
        let dir = temp_fixture("clobber");
        std::fs::write(dir.join("f"), b"x").unwrap();
        let archive = dir.join("out.zip");
        std::fs::write(&archive, b"prior evidence").unwrap();
        let err = collect(
            &[dir.join("f")],
            &archive,
            &ArchiveConfig {
                host: "H".into(),
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("creating archive"), "{err}");
        assert_eq!(std::fs::read(&archive).unwrap(), b"prior evidence");
        std::fs::remove_dir_all(&dir).ok();
    }
}
