use crate::collection_paths::get_paths;
use anyhow::{anyhow, Context};
use log::{debug, error, info, trace, warn};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, ErrorKind, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process;
use zip::CompressionMethod;
mod arguments;
mod collection_paths;
mod platform;
mod version;
extern crate core;
extern crate glob;
extern crate log;
#[cfg(target_os = "windows")]
extern crate winreg;
#[cfg(target_os = "windows")]
use ntfs::indexes::NtfsFileNameIndex;
#[cfg(target_os = "windows")]
use ntfs::structured_values::{
    NtfsAttributeList, NtfsFileName, NtfsFileNamespace, NtfsStandardInformation,
};
#[cfg(target_os = "windows")]
use ntfs::{Ntfs, NtfsAttribute, NtfsAttributeType, NtfsFile, NtfsReadSeek};
use zip::result::ZipResult;

#[cfg(target_os = "windows")]
mod ntfs_driver;
// Compiled for Windows use and exercised by unit tests on all platforms.
#[cfg(any(test, target_os = "windows"))]
mod sector_reader;
#[cfg(target_os = "windows")]
use crate::ntfs_driver::{cd, get};
#[cfg(target_os = "windows")]
use crate::sector_reader::SectorReader;

fn main() -> std::process::ExitCode {
    let cli = arguments::Cli::parse_cli();
    init_logging(&cli);

    match execute(&cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
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

fn execute(cli: &arguments::Cli) -> anyhow::Result<()> {
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

    let created = create_archive(
        archive_path.to_str().context("archive path not UTF-8")?,
        collection_paths,
        &cli.hash_files,
        &dto.zip_level,
    );
    match created {
        Ok(()) => {
            info!("Archive created: {}", archive_path.display());
            Ok(())
        }
        Err(e) => Err(anyhow!("archive creation failed: {e}")),
    }
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

fn create_archive(
    _name: &str,
    _list: Vec<PathBuf>,
    hashing: &bool,
    zip_level: &i32,
) -> ZipResult<()> {
    // TODO - here we have duplicated code - would be useful to abstract this out to prevent duplication.
    // No great rush on that though.
    let path = Path::new(_name);
    let file = File::create(&path);
    let file: File = match file {
        Ok(file) => file,
        Err(e) => panic!("Error on file creation: {}", e),
    };

    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(*zip_level as i64));

    let mut zip = zip::ZipWriter::new(file);
    if *hashing {
        //     if hashing enabled, we want to take a sha256 hash of the file immediately prior to copying
        //     this could be invalid in cases where the file changes regularly, such as event logs. need to explore that further.
        let mut hash_file: File;
        match File::create("tamatoa.txt") {
            Ok(hash_fp) => hash_file = hash_fp,
            Err(e) => {
                debug!("{}", e);
                error!("Error creating file, do you have write privileges to the output folder? Exiting...");
                process::exit(1);
            }
        }
        for i in _list.iter() {
            let input_file: File;
            match File::open(&i) {
                Ok(fp) => input_file = fp,
                Err(e) => {
                    debug!("Unable to access file {:?}. Reason: {}", &i, e);
                    continue;
                }
            }
            let reader = BufReader::new(input_file);
            let digest = sha256_digest(reader).unwrap();

            hash_file
                .write(format!("{}\t{}\n", &digest, &i.to_str().unwrap()).as_bytes())
                .expect(&*format!(
                    "Error writing '{}\t {}' to file",
                    &digest,
                    &i.to_str().unwrap()
                ));
            zip.start_file(i.to_str().unwrap(), *&options)?;

            let mut input_file: File;
            match File::open(&i) {
                Ok(fp) => input_file = fp,
                Err(e) => {
                    debug!("Unable to access file {:?}. Reason: {}", &i, e);
                    continue;
                }
            }

            let mut file_buffer = [0; 1024];

            loop {
                let count = input_file.read(&mut file_buffer).unwrap();
                if count == 0 {
                    break;
                }
                zip.write(&file_buffer[..count])?;
            }
        }
        zip.start_file("tamatoa_hashes.txt", *&options)?;
        let mut input_file = File::open("tamatoa_hashes.txt")?;

        let mut file_buffer = [0; 1024];

        loop {
            let count = input_file.read(&mut file_buffer).unwrap();
            if count == 0 {
                break;
            }
            zip.write(&file_buffer[..count])?;
        }
    } else {
        for i in _list.iter() {
            zip.start_file(i.to_str().unwrap(), *&options)?;
            let mut input_file: File;
            match File::open(&i) {
                Ok(fp) => input_file = fp,
                Err(e) => {
                    debug!(
                        "Unable to access file {:?}. Reason: {}. Trying Low level method.",
                        &i, e
                    );
                    match ll_disk_access(&i) {
                        Ok(fp) => input_file = fp,
                        Err(e) => {
                            debug!("Still unable to access file {:?}. Reason: {}. ", &i, e);
                            continue;
                        }
                    }
                }
            }

            let mut file_buffer = [0; 1024];

            loop {
                let count = input_file.read(&mut file_buffer).unwrap();
                if count == 0 {
                    break;
                }
                zip.write(&file_buffer[..count])?;
            }
        }
    }

    zip.finish()?;
    Ok(())
}

fn sha256_digest<R: Read>(mut reader: R) -> Result<String, ErrorKind> {
    let mut context = Sha256::new();
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(context
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(unix)]
fn ll_disk_access(path: &PathBuf) -> anyhow::Result<File> {
    //     this function is meant to replace the one below on unix like systems.
    //     Realistically, we want to emulate it properly, but as it is root access *should* provide the access we need.
    //  TODO: Implement somethign similar for low leevel access on mac/linux/other supported os
    match File::open(path) {
        Ok(file) => Ok(file),
        Err(e) => {
            error!("Linux/Macos Error: {}", &e);
            Err(anyhow!(e))
        }
    }
}

#[cfg(target_os = "windows")]
fn ll_disk_access(path: &PathBuf) -> anyhow::Result<File> {
    // Get disk label from name
    // prepend \\.
    // Access raw disk
    // Copy file to systemroot/Temp
    // return file access on copied file
    // TODO - come back and finish this off

    let mut file_path = match path
        .to_str()
        .unwrap_or("C:\\")
        .split("\\")
        .collect::<Vec<&str>>()
    {
        path => (path),
    };
    let label_vec = file_path.drain(..1).collect::<Vec<&str>>();
    let label = label_vec
        .first()
        .with_context(|| format!("Error in path!"))?;

    let mut path: Vec<&str>;
    if file_path.len() > 1 {
        path = file_path
            .drain(..file_path.len() - 1)
            .collect::<Vec<&str>>();
    } else {
        path = vec![];
    }

    let filename_vec = file_path.drain(..1).collect::<Vec<&str>>();
    let filename = filename_vec
        .first()
        .with_context(|| format!("Error in filename!"))?;

    let (file_name, data_stream_name) = match filename.find(':') {
        Some(mid) => (&filename[..mid], &filename[mid + 1..]),
        None => (filename.to_owned(), ""),
    };

    let mut raw_path = String::from("\\\\.");
    raw_path.push('\\');
    raw_path.push_str(label);

    let f = File::open(raw_path).unwrap();
    let sr = sector_reader::SectorReader::new(f, 4096)?;
    let mut fs = BufReader::new(sr);

    let mut ntfs = Ntfs::new(&mut fs).unwrap();
    ntfs.read_upcase_table(&mut fs)?;
    let current_directory = vec![ntfs.root_directory(&mut fs)?];

    let mut info = ntfs_driver::CommandInfo {
        current_directory,
        current_directory_string: String::new(),
        fs,
        ntfs: &ntfs,
    };
    for directory in path {
        cd(directory, &mut info).expect("TODO: panic message on directory change");
    }
    match get(file_name, &mut info) {
        Ok(_) => match File::open(file_name) {
            Ok(file) => return Ok(file),
            Err(e) => {
                error!("Unable to copy file {}, reason: {}", &file_name, e);
                Err(anyhow!(e))
            }
        },
        Err(err) => return Err(err),
    }

    // TODO: need to confirm we've tidied up after ourselves here.
}

#[cfg(test)]
mod tests {
    use super::{default_archive_name, resolve_output_path, sanitize_host, sha256_digest};
    use crate::arguments::Cli;
    use clap::error::ErrorKind;
    use clap::Parser;

    #[test]
    fn sha256_digest_matches_known_vector() {
        // KAT cross-checked against `sha256sum` on the same bytes.
        let d = sha256_digest(&b"hello evidence\n"[..]).unwrap();
        assert_eq!(
            d,
            "fe482b5e524c67728f4f2b4f430cd10d9a25659641f995ae537b282ccd181e0b"
        );
    }

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
        let dir = std::env::temp_dir().join(format!("tamatoa-test-join-{}", std::process::id()));
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
