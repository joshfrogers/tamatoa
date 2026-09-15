use crate::archive::{ArchiveConfig, CollectionStats};
use crate::collection_paths::get_paths;
use anyhow::{anyhow, Context};
use log::{error, info, warn};
use std::path::{Path, PathBuf};
mod archive;
mod arguments;
mod collection_paths;
mod platform;
mod version;

#[cfg(target_os = "windows")]
mod ntfs_driver;
#[cfg(target_os = "windows")]
mod rawaccess;
// Compiled for Windows use and exercised by unit tests on all platforms.
#[cfg(any(test, target_os = "windows"))]
mod sector_reader;

/// Orchestrator-facing exit contract:
/// 0 = archive complete; 1 = archive created but some artifacts failed
/// (partial collection); 2 = fatal error, no trustworthy archive.
fn main() -> std::process::ExitCode {
    let cli = arguments::Cli::parse_cli();
    init_logging(&cli);

    match execute(&cli) {
        Ok((stats, sidecar)) => {
            if stats.is_partial() {
                warn!(
                    "partial collection: {} artifacts failed (see manifest)",
                    stats.failed
                );
            }
            if cli.json {
                match std::fs::read_to_string(&sidecar) {
                    Ok(json) => {
                        // Compact single-line JSON on stdout: the orchestrator's machine contract.
                        let v: serde_json::Value = serde_json::from_str(&json)
                            .unwrap_or_else(|_| serde_json::json!({"sidecar": json}));
                        println!("{}", serde_json::to_string(&v).unwrap_or_default());
                    }
                    Err(e) => error!("could not emit JSON summary: {e}"),
                }
            }
            if stats.is_partial() {
                std::process::ExitCode::from(1)
            } else {
                std::process::ExitCode::SUCCESS
            }
        }
        Err(err) => {
            error!("tamatoa failed: {err:#}");
            std::process::ExitCode::from(2)
        }
    }
}

fn init_logging(cli: &arguments::Cli) {
    let level: log::LevelFilter = if cli.quiet {
        log::LevelFilter::Error
    } else {
        cli.verbosity.parse().unwrap_or(log::LevelFilter::Info)
    };
    env_logger::Builder::new()
        .filter_level(level)
        .parse_env("TAMATOA_LOG")
        .format_target(false)
        .init();
}

fn execute(cli: &arguments::Cli) -> anyhow::Result<(CollectionStats, PathBuf)> {
    // Fail fast on bad output targets before paying for path enumeration.
    let archive_path = resolve_output_path(cli)?;
    let dto = arguments::to_dto(cli);
    let collection_paths = get_paths(&dto, &dto.collection_files, &dto.with_usnjrnl)
        .context("resolving collection paths")?;
    if collection_paths.is_empty() {
        return Err(anyhow!("no paths matched for collection"));
    }

    info!(
        "Collecting {} paths into {}",
        collection_paths.len(),
        archive_path.display()
    );

    let cfg = ArchiveConfig {
        host: hostname::get()
            .map(|h| sanitize_host(&h.to_string_lossy()))
            .unwrap_or_else(|_| "unknownhost".to_string()),
        zip_level: cli.zip_level,
        max_file_bytes: cli.max_file_bytes,
        max_total_bytes: cli.max_total_bytes,
        overwrite: cli.force,
    };

    let stats = archive::collect(&collection_paths, &archive_path, &cfg)?;
    let sidecar = archive::sidecar_name(&archive_path);
    info!(
        "Archive created: {} (collected={} failed={} bytes={})",
        archive_path.display(),
        stats.collected,
        stats.failed,
        stats.bytes_archived
    );
    Ok((stats, sidecar))
}

fn resolve_output_path(cli: &arguments::Cli) -> anyhow::Result<PathBuf> {
    let dir = cli.output_dir.clone().unwrap_or_else(|| PathBuf::from("."));
    if !dir.is_dir() {
        return Err(anyhow!("output directory {} does not exist", dir.display()));
    }
    let name = match &cli.output_file {
        Some(n) => n.clone(),
        None => default_archive_name(),
    };
    if Path::new(&name)
        .parent()
        .map_or(false, |p| !p.as_os_str().is_empty())
    {
        return Err(anyhow!(
            "--output-file must be a bare file name, got {name}"
        ));
    }
    let full = dir.join(name);
    if full.exists() && !cli.force {
        return Err(anyhow!(
            "refusing to overwrite existing archive {} (use --force)",
            full.display()
        ));
    }
    Ok(full)
}

fn default_archive_name() -> String {
    let host = hostname::get()
        .map(|h| sanitize_host(&h.to_string_lossy()))
        .unwrap_or_else(|_| "unknownhost".to_string());
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{host}-{:04}{:02}{:02}T{:02}{:02}{:02}Z.zip",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn sanitize_host(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '-' || c == '.');
    if trimmed.is_empty() {
        "unknownhost".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{default_archive_name, resolve_output_path, sanitize_host};
    use crate::arguments::Cli;
    use clap::error::ErrorKind;
    use clap::Parser;

    #[test]
    fn hostnames_are_sanitized_for_filesystem() {
        assert_eq!(sanitize_host("corp\\win-host.local"), "corp-win-host.local");
        assert_eq!(sanitize_host("..."), "unknownhost");
        assert_eq!(sanitize_host(""), "unknownhost");
    }

    #[test]
    fn default_archive_name_carries_host_and_utc_timestamp() {
        let name = default_archive_name();
        assert!(name.ends_with(".zip"), "{name}");
        let stamp = name.rsplit('-').next().unwrap();
        assert_eq!(stamp.len(), "20260102T030405Z.zip".len());
        assert!(stamp[0..8].bytes().all(|b| b.is_ascii_digit()), "{stamp}");
        assert!(stamp.contains("T") && stamp.ends_with("Z.zip"), "{stamp}");
    }

    fn cli_with_dir(dir: &str, extra: &[&str]) -> Cli {
        let mut args = vec!["tamatoa", "-od", dir];
        args.extend_from_slice(extra);
        Cli::try_parse_from(crate::arguments::normalize_args(
            args.into_iter().map(String::from),
        ))
        .unwrap()
    }

    #[test]
    fn output_dir_must_exist() {
        let cli = cli_with_dir("/definitely/not/a/dir", &[]);
        let err = resolve_output_path(&cli).unwrap_err().to_string();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn output_paths_join_instead_of_concat() {
        let dir = std::env::temp_dir().join(format!(
            "tamatoa-test-join-{}-{}",
            std::process::id(),
            file!().len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = cli_with_dir(dir.to_str().unwrap(), &["-of", "out.zip"]);
        let full = resolve_output_path(&cli).unwrap();
        assert_eq!(full, dir.join("out.zip"));

        // existing archive is refused without --force
        std::fs::write(dir.join("out.zip"), b"prior evidence").unwrap();
        let err = resolve_output_path(&cli).unwrap_err().to_string();
        assert!(err.contains("refusing to overwrite"), "{err}");

        let forced = cli_with_dir(dir.to_str().unwrap(), &["-of", "out.zip", "--force"]);
        assert_eq!(resolve_output_path(&forced).unwrap(), dir.join("out.zip"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn output_file_must_be_bare_name() {
        let dir = std::env::temp_dir();
        let cli = cli_with_dir(dir.to_str().unwrap(), &["-of", "../escape.zip"]);
        assert!(resolve_output_path(&cli).is_err());
    }

    #[test]
    fn usage_errors_are_reported_by_clap() {
        let err = Cli::try_parse_from(["tamatoa", "--not-a-flag"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }
}
