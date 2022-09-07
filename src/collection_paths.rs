use crate::cli::Args;
use crate::platform;
use envmnt::{ExpandOptions, ExpansionType};
use glob::MatchOptions;
use glob::Pattern;
use log::{error, info, trace, warn};
use std::path::{Path, PathBuf};
use std::{fs, process};
#[cfg(target_os = "windows")]
use windows_drives;
#[cfg(target_os = "windows")]
use winreg::enums::*;
#[cfg(target_os = "windows")]
use winreg::RegKey;

#[derive(Default, Debug)]
pub struct UserProfile {
    user_key: String,
    path: String,
    profile_path: String,
    full_profile: u32,
}

impl UserProfile {
    pub fn new() -> UserProfile {
        UserProfile {
            user_key: "".to_string(),
            path: "".to_string(),
            profile_path: "".to_string(),
            full_profile: 0,
        }
    }
}

pub fn get_paths(
    cli_args: &Args,
    additional_paths: &Vec<PathBuf>,
    usnjrnl: &bool,
) -> Result<Vec<PathBuf>, glob::PatternError> {
    let mut static_paths: Vec<PathBuf> = additional_paths
        .into_iter()
        .map(|path| PathBuf::from(path))
        .collect();
    let mut glob_paths: Vec<Pattern> = vec![];
    let mut regex_paths: Vec<PathBuf> = vec![];
    let mut collection_paths: Vec<PathBuf> = vec![];

    let case_insensitive: bool = true;

    let mut base_paths: Vec<PathBuf> = vec![];

    if platform::is_unix_like() {
        base_paths.push(PathBuf::from("/"));
    } else {
        //     Determine how we enumerate drives.
        //     We could use unsafe winapi methods but don;t wanna have to deal with that noise.
        //     Another method would be to try to read the root of all drives starting with a character.
        let drive_letters: Vec<&str> = vec![
            "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
            "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
        ];
        for letter in drive_letters {
            let mut drive = PathBuf::from(format!("{}:\\", letter));
            match fs::read_dir(&drive) {
                Ok(_) => base_paths.push(drive),
                Err(why) => {
                    error!("Unable to access {:?}. Reason: {}", &drive, why);
                }
            }
        }
        info!("Found the following drives {:?}", &base_paths)
    }

    if !cli_args.collection_file_path.as_os_str().is_empty() {
        //     IT'S TIME TO GET FUNKY!
        //     Lets try to load a file, and we'll save the patterns from the file if it exists (and has valid patterns)
        error!("Josh hasn't implemented custom file handling yet. go prod him.");
    }

    let collection_files = &cli_args.collection_files.to_owned();

    if collection_files.len() != 0 {
        info!(
            "Adding files {:?}, to custom file collection",
            &cli_args.collection_files
        );
        let files: Vec<PathBuf> = collection_files
            .into_iter()
            .map(|path| PathBuf::from(path))
            .collect();
        static_paths.extend(files);
    }

    let has_mac_folders: bool = Path::new("/Private").exists()
        && Path::new("/Applications").exists()
        && Path::new("/Users").exists();

    if cli_args.collection_file_path.as_os_str().is_empty() || cli_args.collect_defaults {
        info!("Enumerating paths for default artifact collection");

        if !platform::is_unix_like() {
            info!("Windows platform detected");
            let mut exp_options = ExpandOptions::new();
            exp_options.expansion_type = Some(ExpansionType::Windows);
            let system_root = envmnt::expand("%SYSTEMROOT%", Some(exp_options));
            let program_data = envmnt::expand("%PROGRAMDATA%", Some(exp_options));
            let system_drive = envmnt::expand("%SystemDrive%", Some(exp_options));

            glob_paths.push(Pattern::new(&*format!("{}\\Tasks\\**", &system_root))?);
            glob_paths.push(Pattern::new(&*format!("{}\\Prefetch\\**", &system_root))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\System32\\sru\\**",
                &system_root
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\System32\\winevt\\Logs\\**",
                &system_root
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\System32\\Tasks\\**",
                &system_root
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\System32\\LogFiles\\W3SVC1\\**",
                &system_root
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\Appcompat\\Programs\\**",
                &system_root
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\**",
                &program_data
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\$Recycle.Bin\\**\\$I*",
                &system_drive
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "{}\\$Recycle.Bin\\$I*",
                &system_drive
            ))?);

            static_paths.push(PathBuf::from(format!("{}\\SchedLgU.Txt", &system_root)));
            static_paths.push(PathBuf::from(format!(
                "{}\\inf\\setupapi.dev.log",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\drivers\\etc\\hosts",
                &system_root
            )));
            warn!("LiveResponse may currently have  issues collecting SAM, SYSTEM, SOFTWARE, SECURITY");

            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SAM",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SYSTEM",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SOFTWARE",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SECURITY",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SAM.LOG1",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SYSTEM.LOG1",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SOFTWARE.LOG1",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SECURITY.LOG1",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SAM.LOG2",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SYSTEM.LOG2",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SOFTWARE.LOG2",
                &system_root
            )));
            static_paths.push(PathBuf::from(format!(
                "{}\\System32\\config\\SECURITY.LOG2",
                &system_root
            )));

            warn!("LiveResponse may currently have issues collecting $LogFile, $MFT, $UsnJrnl:$J");
            collection_paths.push(PathBuf::from(format!("{}\\$LogFile", &system_drive)));
            collection_paths.push(PathBuf::from(format!("{}\\$MFT", &system_drive)));

            if *usnjrnl {
                collection_paths.push(PathBuf::from(format!(
                    "{}\\$Extend\\$UsnJrnl:$J",
                    &system_drive
                )));
            }

            let users: Vec<UserProfile> = find_users();
            for user in users {
                glob_paths.push(Pattern::new(&*format!(
                    "{}\\AppData\\Roaming\\Microsoft\\Windows\\Recent\\**",
                    user.profile_path
                ))?);
                glob_paths.push(Pattern::new(&*format!(
                    "{}\\AppData\\Local\\Microsoft\\Windows\\WebCache\\**",
                    user.profile_path
                ))?);
                glob_paths.push(Pattern::new(&*format!(
                    "{}\\AppData\\Roaming\\Microsoft\\Windows\\Recent\\AutomaticDestinations\\**",
                    user.profile_path
                ))?);
                glob_paths.push(Pattern::new(&*format!(
                    "{}\\AppData\\Roaming\\Mozilla\\Firefox\\Profiles\\**",
                    user.profile_path
                ))?);
                glob_paths.push(Pattern::new(&*format!(
                    "{}\\AppData\\Local\\ConnectedDevicesPlatform\\**",
                    user.profile_path
                ))?);
                glob_paths.push(Pattern::new(&*format!(
                    "{}\\AppData\\Local\\Microsoft\\Windows\\Explorer\\**",
                    user.profile_path
                ))?);

                static_paths.push(PathBuf::from(format!("{}\\NTUSER.DAT", user.profile_path)));
                static_paths.push(PathBuf::from(format!(
                    "{}\\NTUSER.DAT.LOG1",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!(
                    "{}\\NTUSER.DAT.LOG2",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!(
                    "{}\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!(
                    "{}\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat.LOG1",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!(
                    "{}\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat.LOG2",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!(
                    "{}\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\History",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!(
                    "{}\\AppData\\Local\\Microsoft\\Edge\\User Data\\Default\\History",
                    user.profile_path
                )));
                static_paths.push(PathBuf::from(format!("{}\\AppData\\Roaming\\Microsoft\\Windows\\PowerShell\\PSReadline\\ConsoleHost_history.txt", user.profile_path)));
            }
        } else if platform::is_unix_like() && has_mac_folders {
            info!("macos platform detected");

            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/History*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Cookies*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Bookmarks*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Extensions/**"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Last*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Shortcuts*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Top*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "**/Library/*Support/Google/Chrome/Default/Visited*"
            ))?);
            glob_paths.push(Pattern::new(&*format!("**/places.sqlite*"))?);
            glob_paths.push(Pattern::new(&*format!("**/downloads.sqlite*"))?);
            glob_paths.push(Pattern::new(&*format!("**/*.plist"))?);
            glob_paths.push(Pattern::new(&*format!("/Users/*/.*history"))?);
            glob_paths.push(Pattern::new(&*format!("/root/.*history"))?);
            glob_paths.push(Pattern::new(&*format!("/System/Library/StartupItems/**"))?);
            glob_paths.push(Pattern::new(&*format!("/System/Library/LaunchAgents/**"))?);
            glob_paths.push(Pattern::new(&*format!("/System/Library/LaunchDaemons/**"))?);
            glob_paths.push(Pattern::new(&*format!("/Library/LaunchAgents/**"))?);
            glob_paths.push(Pattern::new(&*format!("/Library/LaunchDaemons/**"))?);
            glob_paths.push(Pattern::new(&*format!("/Library/StartupItems/**"))?);
            glob_paths.push(Pattern::new(&*format!("/var/log/**"))?);
            glob_paths.push(Pattern::new(&*format!("/private/var/log/**"))?);
            glob_paths.push(Pattern::new(&*format!("/private/etc/rc.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/rc.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/.fseventsd/**"))?);

            static_paths.push(PathBuf::from(format!("/etc/hosts.allow")));
            static_paths.push(PathBuf::from(format!("/etc/hosts.deny")));
            static_paths.push(PathBuf::from(format!("/etc/hosts")));
            static_paths.push(PathBuf::from(format!("/private/etc/hosts.allow")));
            static_paths.push(PathBuf::from(format!("/private/etc/hosts.deny")));
            static_paths.push(PathBuf::from(format!("/private/etc/hosts")));
            static_paths.push(PathBuf::from(format!("/etc/passwd")));
            static_paths.push(PathBuf::from(format!("/etc/group")));
            static_paths.push(PathBuf::from(format!("/private/etc/passwd")));
            static_paths.push(PathBuf::from(format!("/private/etc/group")));
        } else if platform::is_unix_like() {
            info!("Linux platform detected");

            // Super user
            static_paths.push(PathBuf::from(format!("/root/.ssh/config")));
            static_paths.push(PathBuf::from(format!("/root/.ssh/known_hosts")));
            static_paths.push(PathBuf::from(format!("/root/.ssh/authorized_keys")));
            static_paths.push(PathBuf::from(format!("/root/.selected_editor")));
            static_paths.push(PathBuf::from(format!("/root/.viminfo")));
            static_paths.push(PathBuf::from(format!("/root/.lesshist")));
            static_paths.push(PathBuf::from(format!("/root/.profile")));
            static_paths.push(PathBuf::from(format!("/root/.selected_editor")));

            // Boot
            static_paths.push(PathBuf::from(format!("/boot/grub/grub.cfg")));
            static_paths.push(PathBuf::from(format!("/boot/grub2/grub.cfg")));

            // Sys
            static_paths.push(PathBuf::from(format!("/sys/firmware/acpi/tables/DSDT")));

            //etc
            static_paths.push(PathBuf::from(format!("/etc/hosts.allow")));
            static_paths.push(PathBuf::from(format!("/etc/hosts.deny")));
            static_paths.push(PathBuf::from(format!("/etc/hosts")));
            static_paths.push(PathBuf::from(format!("/etc/passwd")));
            static_paths.push(PathBuf::from(format!("/etc/group")));
            static_paths.push(PathBuf::from(format!("/etc/crontab")));
            static_paths.push(PathBuf::from(format!("/etc/cron.allow")));
            static_paths.push(PathBuf::from(format!("/etc/cron.deny")));
            static_paths.push(PathBuf::from(format!("/etc/anacrontab")));
            static_paths.push(PathBuf::from(format!("/var/spool/anacron/cron.daily")));
            static_paths.push(PathBuf::from(format!("/var/spool/anacron/cron.hourly")));
            static_paths.push(PathBuf::from(format!("/var/spool/anacron/cron.weekly")));
            static_paths.push(PathBuf::from(format!("/var/spool/anacron/cron.monthly")));
            static_paths.push(PathBuf::from(format!("/etc/apt/sources.list")));
            static_paths.push(PathBuf::from(format!("/etc/apt/trusted.gpg")));
            static_paths.push(PathBuf::from(format!("/etc/apt/trustdb.gpg")));
            static_paths.push(PathBuf::from(format!("/etc/resolv.conf")));
            static_paths.push(PathBuf::from(format!("/etc/fstab")));
            static_paths.push(PathBuf::from(format!("/etc/issues")));
            static_paths.push(PathBuf::from(format!("/etc/issues.net")));
            static_paths.push(PathBuf::from(format!("/etc/insserv.conf")));
            static_paths.push(PathBuf::from(format!("/etc/localtime")));
            static_paths.push(PathBuf::from(format!("/etc/timezone")));
            static_paths.push(PathBuf::from(format!("/etc/pam.conf")));
            static_paths.push(PathBuf::from(format!("/etc/rsyslog.conf")));
            static_paths.push(PathBuf::from(format!("/etc/xinetd.conf")));
            static_paths.push(PathBuf::from(format!("/etc/netgroup")));
            static_paths.push(PathBuf::from(format!("/etc/nsswitch.conf")));
            static_paths.push(PathBuf::from(format!("/etc/ntp.conf")));
            static_paths.push(PathBuf::from(format!("/etc/yum.conf")));
            static_paths.push(PathBuf::from(format!("/etc/chrony.conf")));
            static_paths.push(PathBuf::from(format!("/etc/chrony")));
            static_paths.push(PathBuf::from(format!("/etc/sudoers")));
            static_paths.push(PathBuf::from(format!("/etc/logrotate.conf")));
            static_paths.push(PathBuf::from(format!("/etc/environment")));
            static_paths.push(PathBuf::from(format!("/etc/hostname")));
            static_paths.push(PathBuf::from(format!("/etc/host.conf")));
            static_paths.push(PathBuf::from(format!("/etc/fstab")));
            static_paths.push(PathBuf::from(format!("/etc/machine-id")));
            static_paths.push(PathBuf::from(format!("/etc/screen-rc")));

            // User profiles
            glob_paths.push(Pattern::new(&*format!("/home/*/.*history"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.ssh/known_hosts"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.ssh/config"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.ssh/autorized_keys"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.viminfo"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.profile"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.*rc"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.*_logout"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.selected_editor"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.wget-hsts"))?);
            glob_paths.push(Pattern::new(&*format!("/home/*/.gitconfig"))?);

            // Firefox artifacts
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.mozilla/firefox/*.default*/**/*.sqlite*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.mozilla/firefox/*.default*/**/*.json"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.mozilla/firefox/*.default*/**/*.txt"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.mozilla/firefox/*.default*/**/*.db*"
            ))?);

            // Chrome artifacts
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/History*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Cookies*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Bookmarks*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Extensions/**"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Last*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Shortcuts*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Top*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Visited*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Preferences*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Login Data*"
            ))?);
            glob_paths.push(Pattern::new(&*format!(
                "/home/*/.config/google-chrome/Default/Web Data*"
            ))?);

            // Superuser profiles
            glob_paths.push(Pattern::new(&*format!("/root/.*history"))?);
            glob_paths.push(Pattern::new(&*format!("/root/.*rc"))?);
            glob_paths.push(Pattern::new(&*format!("/root/.*_logout"))?);

            // var
            glob_paths.push(Pattern::new(&*format!("/var/log/**"))?);
            glob_paths.push(Pattern::new(&*format!("/var/spool/at/**"))?);
            glob_paths.push(Pattern::new(&*format!("/var/spool/cron/**"))?);

            // etc
            glob_paths.push(Pattern::new(&*format!("/etc/rc.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/cron.daily/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/cron.hourly/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/cron.weekly/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/cron.monthly/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/modprobe.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/modprobe-load.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/*-release"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/pam.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/rsyslog.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/yum.repos.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/init.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/systemd.d/**"))?);
            glob_paths.push(Pattern::new(&*format!("/etc/default/**"))?);
        } else {
            error!("This is an unsupported platform, go fix this.");
            return Ok(vec![]);
        }

        let mut num_paths: i32 = 0;
        info!("Enumerating file systems and matching patterns");
        for base_path in base_paths {
            info!("Enumerating volume: {:?}", &base_path);
            for entry in walk_tree(base_path) {
                num_paths += 1;
                if static_paths.contains(&entry) {
                    collection_paths.push(entry);
                    continue;
                }

                let mut globfound: bool = false;
                for globpattern in &glob_paths {
                    let glob_entry = &entry;
                    let glob_options = MatchOptions {
                        case_sensitive: false,
                        require_literal_separator: false,
                        require_literal_leading_dot: false,
                    };
                    globfound = globpattern.matches_path_with(glob_entry, glob_options);
                    if globfound {
                        collection_paths.push(glob_entry.to_owned());
                        break;
                    }
                }
                if globfound {
                    continue;
                }

                //     Extend this to regex matching too.
                //     only used if we have custom code
            }
            info!("Scanned {} paths", num_paths);
        }
    }
    info!("Found {} paths to collect", collection_paths.len());
    Ok(collection_paths)
}

pub fn walk_tree(base_path: PathBuf) -> Vec<PathBuf> {
    let mut dir_stack: Vec<PathBuf> = vec![];
    let mut file_listing: Vec<PathBuf> = vec![];
    dir_stack.push(base_path);
    while dir_stack.len() > 0 {
        // Need to go through and handle the erroring paths here
        if let Some(dir) = dir_stack.pop() {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries {
                    if let Ok(path_entry) = entry {
                        let path = path_entry.path();
                        // Doesn't handle symlinks by default. Let's see if we can sort this out.
                        if path.is_dir() {
                            // check metadata to see if it is a symlink, if so, dont follow it.
                            if let Ok(is_symlink) = fs::symlink_metadata(&path) {
                                if !is_symlink.file_type().is_symlink() {
                                    dir_stack.push(path);
                                }
                            }
                        } else {
                            file_listing.push(path);
                        }
                    }
                }
            }
        }
    }
    file_listing
}

#[cfg(unix)]
pub fn find_users() -> Vec<UserProfile> {
    vec![]
}

#[cfg(target_os = "windows")]
pub fn find_users() -> Vec<UserProfile> {
    let hklm: RegKey = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList");

    let mut user_vec: Vec<UserProfile> = vec![];

    return if let Ok(registry_key) = key {
        let profile_names = registry_key.enum_keys();

        for user in profile_names {
            if let Ok(true_user) = user {
                let path = format!(
                    "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\{}",
                    &true_user
                );
                let profile = hklm.open_subkey(&path);
                if let Ok(working_profile) = profile {
                    let mut profile_path: String = String::from("");
                    if let Ok(ppath) = working_profile.get_value("ProfileImagePath") {
                        profile_path = ppath;
                    }
                    let mut full_profile: u32 = 0;
                    if let Ok(fprofile) = working_profile.get_value("FullProfile") {
                        full_profile = fprofile;
                        let result = UserProfile {
                            user_key: true_user,
                            path: format!("HKEY_LOCAL_MACHINE\\{}\\ProfileImagePath", &path),
                            profile_path,
                            full_profile,
                        };

                        user_vec.push(result);
                    }
                }
            }
        }
        user_vec
    } else {
        error!(
            "{}",
            String::from("Unable to access profile list registry key.")
        );
        user_vec
    };
}
