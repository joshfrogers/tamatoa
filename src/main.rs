use crate::collection_paths::get_paths;
use anyhow::{anyhow, Context as anyhow_context};
use env_logger::Env;
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
#[cfg(target_os = "windows")]
mod sector_reader;
#[cfg(target_os = "windows")]
use crate::ntfs_driver::{cd, get};
#[cfg(target_os = "windows")]
use crate::sector_reader::SectorReader;

fn main() {
    let cli_args: arguments::CLIArguments = arguments::CLIArguments::new();

    let mut log_level: String;
    if cli_args.disable_logging {
        log_level = "none".to_string();
    } else if !cli_args.log_verbosity.is_empty() {
        println!("Running with log verbosity: {}", &cli_args.log_verbosity);
        log_level = cli_args.log_verbosity.clone();
    } else {
        log_level = "warn".to_string();
    }

    let env = Env::default()
        .filter_or("LRLOGLEVEL", log_level)
        .write_style_or("LRLOGSTYLE", "always");

    env_logger::init_from_env(env);

    let mut collection_paths: Vec<PathBuf> = vec![];
    match get_paths(&cli_args, &cli_args.collection_files, &cli_args.usnjrnl) {
        Ok(path_vector) => collection_paths = path_vector,
        Err(e) => error!("Error in collecting paths: {}", e),
    }

    info!(
        "Collecting and writing {} files to zip",
        &collection_paths.len()
    );

    let zip_filename: &str = &cli_args.output_filename;
    let zip_path: String = cli_args.output_path + zip_filename;

    match create_archive(
        zip_path.as_str(),
        collection_paths,
        &cli_args.hash_files,
        &cli_args.zip_level,
    ) {
        Ok(_) => trace!("Archive successfully created."),
        Err(_) => error!("Archive creation error!"),
    }
    // the above method doesn't always let us copy data that is is use (or may be locked to admin perms)
    // methods we could look into:
    //  - NTFS crate
    //  - VSS copy
    //  - dump raw contents from disk (would likely be unsafe, not sure I want to go down that path, but perhaps we could expose via a switch?)
    //  - 'unlock' the file - which may have non-benign consequences.

    //     include the log messages within the archive
    // println!("{:?}", collection_paths::find_users());
    //     set up the connection to the sftp server, if arguments provided imply we're setting one up.
    //     connect to the SFTP server
    //     Create the file stream
    if false {
        if cli_args.use_sftp {
            info!("Following is mocked connection:");
            info!(
                "Connecting to server: {}, using credentials {}:{}",
                cli_args.sftp_server, cli_args.user_name, cli_args.user_password
            );
            info!(
                "SFTP settings - outputpath: {}, cleanup: {}, dry_run: {}",
                cli_args.sftp_output_path, cli_args.sftp_cleanup, cli_args.dry_run
            );
        }
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
    use super::sha256_digest;

    #[test]
    fn sha256_digest_matches_known_vector() {
        // KAT cross-checked against `sha256sum` on the same bytes.
        let d = sha256_digest(&b"hello evidence\n"[..]).unwrap();
        assert_eq!(
            d,
            "fe482b5e524c67728f4f2b4f430cd10d9a25659641f995ae537b282ccd181e0b"
        );
    }
}
