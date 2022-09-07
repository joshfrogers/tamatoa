use crate::errors::ErrCode;
use crate::platform;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "tamatoa", about, author)]
pub struct Args {
    #[structopt(
        parse(from_os_str),
        short,
        long = "collection_file_config",
        default_value = "",
        help = "Optional argument to provide custom list of artifact files and directories (one entry per line). NOTE: Please see CUSTOM_PATH_TEMPLATE.txt for sample.\nUsage: -c <path to config file>"
    )]
    pub collection_file_path: PathBuf,

    #[structopt(
        long = "collect_defaults",
        help = "If custom collection paths have been defined, also collect default paths?"
    )]
    pub collect_defaults: bool,

    #[structopt(
        parse(from_os_str),
        short = "f",
        long = "files",
        default_value = "",
        help = "Collect additional files. Use multiple times for multiple files.\nUsage -f <filepath> -f <filepath2>"
    )]
    pub collection_files: Vec<PathBuf>,

    #[structopt(
        parse(from_os_str),
        short = "o",
        long = "output-path",
        default_value = "./",
        help = "Defines the directory that the zip archive will be created in. Defaults to current working directory.\nUsage: -output-path <directory path>"
    )]
    pub output_path: PathBuf,

    #[structopt(
        parse(from_os_str),
        long = "output-filename",
        default_value = "tamatoa.zip",
        help = "Defines the name of the zip archive will be created. Defaults to host machine's name.\nUsage: -output-filename <archive name>"
    )]
    pub output_filename: PathBuf,

    #[structopt(
        long = "sftp",
        help = "Use an sftp server to upload the collected artifact zip file."
    )]
    pub use_sftp: bool,

    #[structopt(long = "sftp-user", default_value = "", help = "SFTP username")]
    pub sftp_username: String,

    #[structopt(long = "sftp-pass", default_value = "", help = "SFTP password")]
    pub sftp_password: String,

    #[structopt(
        long = "sftp-server",
        default_value = "",
        help = "SFTP Server resolvable hostname or IP address and port. If no port is given then 22 is used by default.  Format is <server name>:<port>\n Usage: --sftp-server <ip>:<port>"
    )]
    pub sftp_server: String,

    #[structopt(
        parse(from_os_str),
        long = "sftp-paths",
        default_value = "",
        help = "Defines the output directory on the SFTP server, as it may be a different location than the ZIP generate on disk. Can be full or relative path.\n Usage: --sftp-paths <directory path>"
    )]
    pub sftp_output_path: PathBuf,

    #[structopt(
        long = "no-sftp-cleanup",
        help = "Disables the removal of the .zip file used for collection after uploading to the SFTP server. Only applies if SFTP option is enabled.\n Usage: --no-sftpcleanup"
    )]
    pub sftp_cleanup: bool,

    #[structopt(
        short,
        long,
        help = "Collect artifacts to a virtual zip archive, but does not send or write to disk."
    )]
    pub dry_run: bool,

    #[structopt(
        long,
        help = "Uses the native file system instead of a raw NTFS read. Unix-like environments always use this option."
    )]
    pub force_native: bool,

    #[structopt(
        long,
        default_value = "6",
        help = "Takes a number between 1-9 to change the compression level of the archive file.\nUsage: --zip-level 6"
    )]
    pub zip_level: i32,

    #[structopt(long, help = "Enables collecting $UsnJrnl")]
    pub usnjrnl: bool,

    // #[structopt(skip, parse(from_os_str), long, default_value = "./", help="Sets the file path to write log messages to. Defaults to ./tamatoa.log\n Usage: --log-file-path tamatoa.log")]
    // pub log_file_path: PathBuf,
    #[structopt(
        short = "v",
        long,
        default_value = "info",
        help = "Set verbosity of the console log. By default the console only shows information or greater events and the file log shows all entries. Disabled when `-q` is used.\n Usage: -v [debug, info, warn]"
    )]
    pub log_verbosity: String,

    #[structopt(
        short = "q",
        long,
        help = "Disables logging to the console and file.\n Usage: -q"
    )]
    pub disable_logging: bool,

    #[structopt(
        short = "hf",
        long,
        help = "Generate a hash for each of the files being collected - uses SHA256 algorithm to generate a hash of the file. WARNING: Will increase execution time."
    )]
    pub hash_files: bool,
}

pub fn parse_args() -> Result<Args, ErrCode> {
    let mut args = Args::from_args();

    if !args.disable_logging {
        if args.log_verbosity == String::from("info") {
            setup_log(log::LevelFilter::Info);
        } else if args.log_verbosity == String::from("debug") {
            setup_log(log::LevelFilter::Debug)
        } else {
            setup_log(log::LevelFilter::Warn);
        }
    }

    if args.force_native && !platform::supports_raw_access() {
        log::warn!(
            "Warning: This platform only supports native reads, --force-native has no effect."
        );
    }

    if args.zip_level < 1 || args.zip_level > 9 {
        return Err(ErrCode::ZipError(1))
    }

    Ok(args)
}

pub fn setup_log(level: log::LevelFilter) {
    env_logger::Builder::from_default_env()
        .format_timestamp_secs()
        .filter(None, level)
        .init();
}
