use crate::platform;
use crate::version::get_version;
use std::collections::HashMap;
use std::{env, process};

#[derive(Default, Debug)]
pub struct CLIArguments {
    pub(crate) collection_file_path: String,
    pub(crate) collect_defaults: bool,
    pub(crate) collection_files: Vec<String>,
    pub(crate) output_path: String,
    pub(crate) output_filename: String,
    pub(crate) use_sftp: bool,
    pub(crate) user_name: String,
    pub(crate) user_password: String,
    pub(crate) sftp_server: String,
    pub(crate) sftp_output_path: String,
    pub(crate) sftp_cleanup: bool,
    pub(crate) dry_run: bool,
    pub(crate) force_native: bool,
    pub(crate) zip_level: i32,
    pub(crate) usnjrnl: bool,
    // pub(crate) log_file_path: String,
    pub(crate) log_verbosity: String,
    pub(crate) disable_logging: bool,
    pub(crate) hash_files: bool,
}

impl CLIArguments {
    pub fn new() -> CLIArguments {
        // TODO
        let mut force_native: bool = !platform::supports_raw_access();
        let mut help_requested: bool = false;
        let mut collection_file_path: String = String::from("");
        let mut collect_defaults: bool = false;
        let mut collection_files: Vec<String> = vec![String::from("")];
        let mut output_path: String = String::from("./");
        let mut output_filename: String = String::from("tamatoa.zip");
        let mut use_sftp: bool = false;
        let mut user_name: String = String::from("");
        let mut user_password: String = String::from("");
        let mut sftp_server: String = String::from("");
        let mut sftp_output_path: String = String::from("");
        let mut sftp_cleanup: bool = false;
        let mut dry_run: bool = false;
        let mut zip_level: i32 = 6;
        let mut usnjrnl: bool = false;
        // let mut log_file_path: String = String::from("");
        let mut log_verbosity: String = String::from("info");
        let mut disable_logging: bool = false;
        let mut hash_files: bool = false;

        println!("tamatoa version {}", get_version());

        let args: Vec<String> = env::args().collect::<Vec<String>>()[1..].to_vec();

        if args.len() < 1 {
            arg_help()
        }

        let mut iter_args = args.iter().peekable();
        // iterate through the vector and switch match on results
        // If arguments have not been set, we should manually set them to defaults.
        // can we set default values in a struct?
        // we can - impl Default for CLIArguments, or  we can derive default

        while let Some(item) = iter_args.next() {
            if iter_args.peek().is_none() {
                break;
            }
            match item.as_ref() {
                "--help" => help_requested = true,
                "-h" => help_requested = true,
                "/?" => help_requested = true,
                "--version" => help_requested = true,
                "-od" => {
                    output_path = match iter_args.peek() {
                        Some(path) => check_real_arg(path.to_owned().to_owned(), output_path),
                        None => String::from("./"),
                    }
                }
                "-of" => {
                    output_filename = match iter_args.peek() {
                        Some(path) => check_real_arg(path.to_owned().to_owned(), output_filename),
                        None => String::from("sac.zip"),
                    }
                }
                "-c" => {
                    collection_file_path = match iter_args.peek() {
                        Some(path) => {
                            check_real_arg(path.to_owned().to_owned(), collection_file_path)
                        }
                        None => String::from(""),
                    }
                }
                "-d" => {
                    collection_file_path = match iter_args.peek() {
                        Some(path) => {
                            check_real_arg(path.to_owned().to_owned(), collection_file_path)
                        }
                        None => String::from(""),
                    };
                    collect_defaults = true
                }
                "--sftp" => use_sftp = true,
                "-u" => {
                    user_name = match iter_args.peek() {
                        Some(name) => check_real_arg(name.to_owned().to_owned(), user_name),
                        None => String::from(""),
                    }
                }
                "-p" => {
                    user_password = match iter_args.peek() {
                        Some(password) => {
                            check_real_arg(password.to_owned().to_owned(), user_password)
                        }
                        None => String::from(""),
                    }
                }
                "-s" => {
                    sftp_server = match iter_args.peek() {
                        Some(server) => check_real_arg(server.to_owned().to_owned(), sftp_server),
                        None => String::from(""),
                    }
                }
                "-os" => {
                    sftp_output_path = match iter_args.peek() {
                        Some(output_path) => {
                            check_real_arg(output_path.to_owned().to_owned(), sftp_output_path)
                        }
                        None => String::from(""),
                    }
                }
                "--no-sftpcleanup" => sftp_cleanup = false,
                "--dry-run" => dry_run = true,
                "--force-native" => {
                    if force_native {
                        println!("Warning: This platform only supports native reads, --force-native has no effect.")
                    } else {
                        force_native = true
                    }
                }
                "-zl" => {
                    zip_level = match iter_args.peek() {
                        Some(zip_level) => match zip_level.to_owned().to_owned().parse() {
                            Ok(level) => level,
                            Err(e) => {
                                println!(
                                    "Unable to parse string, {}. Defaulting to ziplevel 6.",
                                    e
                                );
                                6
                            }
                        },
                        None => 6,
                    };
                }
                "--usnjrnl" => usnjrnl = true,
                // "-l" => {
                //     log_file_path = match iter_args.peek() {
                //         Some(log_path) => log_path.to_owned().to_owned(),
                //         None => String::from("."),
                //     }
                // }
                "-q" => disable_logging = true,
                "-hf" => hash_files = true,
                "-run" => continue,
                "-v" => {
                    log_verbosity = match iter_args.peek() {
                        Some(log_v) => check_real_arg(
                            log_v.to_owned().to_owned().to_lowercase(),
                            log_verbosity,
                        ),
                        None => String::from("info"),
                    }
                }
                _ => collection_files.push(item.to_string()),
            }
        }

        if !vec!["trace", "info", "warn", "error", "none"].contains(&&*log_verbosity) {
            log_verbosity = String::from("info");
        }

        if help_requested {
            arg_help()
        }

        CLIArguments {
            collection_file_path,
            collect_defaults,
            collection_files,
            output_path,
            output_filename,
            use_sftp,
            user_name,
            user_password,
            sftp_server,
            sftp_output_path,
            sftp_cleanup,
            dry_run,
            force_native,
            zip_level,
            usnjrnl,
            // log_file_path,
            log_verbosity,
            disable_logging,
            hash_files,
        }
    }
}

fn check_real_arg(arg_value: String, default: String) -> String {
    // Checking to ensure we're not taking in arguments as values for arguments.
    match arg_value.as_str() {
        "--help" => default,
        "-h" => default,
        "/?" => default,
        "--version" => default,
        "-od" => default,
        "-of" => default,
        "-c" => default,
        "-d" => default,
        "--sftp" => default,
        "-u" => default,
        "-p" => default,
        "-s" => default,
        "-os" => default,
        "--no-sftpcleanup" => default,
        "--dry-run" => default,
        "--force-native" => default,
        "-zl" => default,
        "--usnjrnl" => default,
        "-l" => default,
        "-q" => default,
        "-hf" => default,
        "-run" => default,
        "-v" => default,
        _ => arg_value,
    }
}

fn arg_help() {
    let filepath: String;
    match env::current_exe() {
        Ok(exe_path) => {
            filepath = match exe_path.into_os_string().into_string() {
                Ok(exe_path) => exe_path,
                Err(e) => {
                    println!("Unable to convert path from os_string: {:?}", e);
                    String::from("tamatoa")
                }
            }
        }
        Err(e) => {
            println!("Defaulting to standard filename 'tamatoa[.exe]', {}", e);
            filepath = String::from("tamatoa")
        }
    };

    let output = format!("\n\nUsage: {} [Options]... [Files]...\n\nThe tamatoa tool collects forensic artifacts from hosts with NTFS file systems quickly, securely and minimizes impact to the host.\n\nThe available options are:", filepath);
    println!("{}", output);
    println!("");
    let help_topics = HashMap::from([
       ("-od",
            "Defines the directory that the zip archive will be created in. Defaults to current working directory.\n\tUsage: -od <directory path>"),
       ("-of",
            "Defines the name of the zip archive will be created. Defaults to host machine's name.\n\tUsage: -of <archive name>"),
       (
            "-c",
            "Optional argument to provide custom list of artifact files and directories (one entry per line). NOTE: Please see CUSTOM_PATH_TEMPLATE.txt for sample.\n\tUsage: -c <path to config file>"
       ),
       (
            "-d",
            "Same as '-c' but will collect default paths included in tamatoa in addition to those specified in the provided config file.\n\tUsage: -d <path to config file>"
       ),
       // (
       //      "-u",
       //      "SFTP username"
       //  ),
       //  (
       //      "-p",
       //      "SFTP password"
       //  ),
       //  (
       //      "-s",
       //      "SFTP Server resolvable hostname or IP address and port. If no port is given then 22 is used by default.  Format is <server name>:<port>\n\t Usage: -s <ip>:<port>"
       //  ),
       //  (
       //      "-os",
       //      "Defines the output directory on the SFTP server, as it may be a different location than the ZIP generate on disk. Can be full or relative path.\n\t Usage: -os <directory path>"
       //  ),
       //  (
       //      "--no-sftpcleanup",
       //      "Disables the removal of the .zip file used for collection after uploading to the SFTP server. Only applies if SFTP option is enabled.\n\t Usage: --no-sftpcleanup"
       //  ),
       //  (
       //      "--dry-run",
       //      "Collect artifacts to a virtual zip archive, but does not send or write to disk."
       //  ),
        // (
        //     "--force-native",
        //     "Uses the native file system instead of a raw NTFS read. Unix-like environments always use this option."
        // ),
        // (
        //     "-zp",
        //     "Uses a password to encrypt the archive file"
        // ),
        // (
        //     "-zl",
        //     "Uses a number between 1-9 to change the compression level of the archive file"
        // ),
        (
            "--usnjrnl",
            "Enables collecting $UsnJrnl"
        ),
        // (
        //     "-l",
        //     "Sets the file path to write log messages to. Defaults to ./tamatoa.log\n\t Usage: -l tamatoa.log"
        // ),
        (
            "-q",
            "Disables logging to the console and file.\n\t Usage: -q"
        ),
        (
            "-v",
            "Set verbosity of the console log. By default the console only shows information or greater events and the file log shows all entries. Disabled when `-q` is used.\n\t Usage: -v [trace, info, warn, error, none]"
        ),
        (
            "-hf",
            "Generate a hashes for each of the files being collected - uses SHA256 algorithm to generate a hash of the files. WARNING: will increasee execution time."
        )
    ]);
    for (key, value) in help_topics.into_iter() {
        println!("{}\t\t{}", key, value);
    }
    process::exit(1);
}
