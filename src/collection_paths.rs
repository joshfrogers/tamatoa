//! Path discovery. The old implementation walked every mounted volume into a
//! Vec and matched every scanned path against every pattern - a full-disk
//! enumeration for what is, at runtime, a few dozen targeted glob lookups.
//!
//! Now: each default/config entry is resolved *directly* - glob entries stream
//! through `glob::glob_with`, plain files are existence-checked, plain
//! directories are walked (symlinks never followed, pseudo-filesystems
//! pruned). Work is proportional to what is collected, not to the size of the
//! disk. Results are deduplicated and ordered deterministically.

use crate::arguments::CLIArguments;
use anyhow::{Context, Result};
use glob::{glob_with, MatchOptions};
use log::{debug, error, info, warn};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use winreg::enums::*;
#[cfg(target_os = "windows")]
use winreg::RegKey;

#[derive(Default, Debug)]
pub struct UserProfile {
    pub user_key: String,
    pub path: String,
    pub profile_path: String,
    pub full_profile: u32,
}

/// Top-level pseudo/volatile filesystems never contain collectable evidence on
/// Linux/macOS; walking or globbing into them wastes minutes and can hit
/// unbounded virtual files.
const PRUNED_DIRS: &[&str] = &["/proc", "/sys", "/dev", "/run", "/snap", "/system"];

fn match_options() -> MatchOptions {
    MatchOptions {
        // Windows filesystems are case-insensitive by default; POSIX is
        // case-sensitive and evidence paths should be matched exactly.
        case_sensitive: !cfg!(target_os = "windows"),
        // `*` must not cross directory boundaries; only `**` descends.
        require_literal_separator: true,
        require_literal_leading_dot: false,
    }
}

/// Does this entry contain glob metacharacters?
fn is_glob(entry: &str) -> bool {
    entry.contains('*') || entry.contains('?')
}

/// Expand one collection entry (literal path, directory, or glob pattern) and
/// append every resulting file path to `out` (deduplicated via `seen`).
pub fn add_entry(raw_entry: &str, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let entry = raw_entry.trim();
    if entry.is_empty() || entry.starts_with('#') {
        return;
    }

    if is_glob(entry) {
        // The glob crate speaks `/`; Windows callers build patterns with `\`.
        let pattern = entry.replace('\\', "/");
        match glob_with(&pattern, match_options()) {
            Ok(paths) => {
                for path in paths {
                    match path {
                        Ok(p) => push_if_file_or_walk(p, out, seen),
                        Err(e) => debug!("glob error for {pattern}: {e}"),
                    }
                }
            }
            Err(e) => warn!("invalid glob pattern '{entry}': {e}"),
        }
        return;
    }

    push_if_file_or_walk(PathBuf::from(entry), out, seen);
}

/// Files are collected directly; directories are expanded with a pruned walk;
/// anything else is left for the collector's preflight to classify (so
/// symlinks/special files end up in the manifest with reasons, not silently).
fn push_if_file_or_walk(path: PathBuf, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    // symlink_metadata: never follow, so a symlinked directory is not walked.
    let md = match fs::symlink_metadata(&path) {
        Ok(md) => md,
        Err(_) => {
            // Missing or unreadable metadata: record it anyway when it at least
            // looks like a concrete path, so the manifest logs the failure.
            if path.parent().map(|p| p.exists()).unwrap_or(false) {
                if seen.insert(path.clone()) {
                    out.push(path);
                }
            } else {
                debug!("skipping nonexistent entry {}", path.display());
            }
            return;
        }
    };
    let ft = md.file_type();
    if ft.is_dir() {
        for file in walk_tree(path) {
            if seen.insert(file.clone()) {
                out.push(file);
            }
        }
    } else if seen.insert(path.clone()) {
        out.push(path);
    } else {
        debug!("deduplicated {}", path.display());
    }
}

/// Depth-first file listing. Uses DirEntry metadata (one stat per entry, no
/// follow), skips symlinks, and prunes pseudo-filesystems.
pub fn walk_tree(base_path: PathBuf) -> Vec<PathBuf> {
    let mut dir_stack: Vec<PathBuf> = vec![base_path];
    let mut file_listing: Vec<PathBuf> = vec![];
    while let Some(dir) = dir_stack.pop() {
        if pruned(&dir) {
            debug!("pruned {}", dir.display());
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            debug!("unreadable directory {}", dir.display());
            continue;
        };
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            let path = entry.path();
            if ft.is_symlink() {
                continue; // never followed
            }
            if ft.is_dir() {
                if !pruned(&path) {
                    dir_stack.push(path);
                }
            } else if ft.is_file() {
                file_listing.push(path);
            }
            // sockets/fifos/devices inside directories are not collectable
        }
    }
    file_listing
}

fn pruned(dir: &Path) -> bool {
    PRUNED_DIRS
        .iter()
        .any(|p| dir.as_os_str() == std::ffi::OsStr::new(p))
}

/// Parse a tamatoa config file: one entry (file, directory or glob) per line,
/// `#` comments, `$VAR`/`${VAR}`/`%VAR%` environment expansion.
pub fn parse_config(path: &Path) -> Result<Vec<String>> {
    let bytes = fs::read(path).with_context(|| format!("reading config {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let entries = text
        .lines()
        .map(|line| expand_env(line.trim()))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    Ok(entries)
}

/// Expand `$NAME`, `${NAME}` and `%NAME%` references from the process
/// environment. Unknown references expand to an unmatchable marker so the
/// entry fails loudly in the manifest rather than silently matching junk.
fn expand_env(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < input.len() {
        let b = bytes[i];
        if b == b'$'
            && i + 1 < bytes.len()
            && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_')
        {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            push_var(&input[start..end], &mut out);
            i = end;
        } else if b == b'$' && i + 2 < bytes.len() && bytes[i + 1] == b'{' {
            // ${NAME}
            let name_start = i + 2;
            let mut end = name_start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'}' && end > name_start {
                push_var(&input[name_start..end], &mut out);
                i = end + 1;
            } else {
                out.push('$');
                i += 1;
            }
        } else if b == b'%' {
            // %NAME%
            let name_start = i + 1;
            let close = input[name_start..].find('%').map(|c| name_start + c);
            if let Some(close) = close {
                let name = &input[name_start..close];
                if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    push_var(name, &mut out);
                    i = close + 1;
                    continue;
                }
            }
            out.push('%');
            i += 1;
        } else {
            let ch = input[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn push_var(name: &str, out: &mut String) {
    match std::env::var(name) {
        Ok(v) => out.push_str(&v),
        Err(_) => {
            // Marker that can never match a real path; the manifest records it.
            out.push_str("%UNSET-");
            out.push_str(name);
            out.push('%');
        }
    }
}

/// A Windows env-var derived collection base must look like an absolute path,
/// or every pattern built from it is garbage (the historical bug class here).
#[cfg(target_os = "windows")]
fn validated_env_base(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(v) if v.len() >= 3 && v.as_bytes()[1] == b':' => v,
        Ok(v) => {
            warn!("{name}={v:?} is not an absolute path; using {default:?}");
            default.to_string()
        }
        Err(_) => {
            warn!("{name} unset; using {default:?}");
            default.to_string()
        }
    }
}

pub fn get_paths(
    cli_args: &CLIArguments,
    additional_paths: &[String],
    usnjrnl: &bool,
) -> Result<Vec<PathBuf>> {
    let mut collection_paths: Vec<PathBuf> = vec![];
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // 1. Config file entries (-c replaces defaults, -d adds to them).
    for (path, label) in [
        (cli_args.collection_file_path.as_str(), "-c"),
        (cli_args.defaults_config_path.as_str(), "-d"),
    ] {
        if path.is_empty() {
            continue;
        }
        let entries = parse_config(Path::new(path))
            .with_context(|| format!("loading config file ({label})"))?;
        info!(
            "Loaded {} entries from config {label} ({path})",
            entries.len()
        );
        for entry in entries {
            add_entry(&entry, &mut collection_paths, &mut seen);
        }
    }

    // 2. Positional paths always collect, with or without a config file.
    for entry in additional_paths {
        add_entry(entry, &mut collection_paths, &mut seen);
    }

    // 3. Default artifact sets.
    if cli_args.collection_file_path.is_empty() || cli_args.collect_defaults {
        info!("Enumerating paths for default artifact collection");
        if cfg!(target_os = "windows") {
            default_windows(&mut collection_paths, &mut seen, *usnjrnl);
        } else if mac_folders() {
            info!("macOS platform detected");
            default_mac(&mut collection_paths, &mut seen);
        } else if cfg!(unix) {
            info!("Linux platform detected");
            default_linux(&mut collection_paths, &mut seen);
        } else {
            error!("This is an unsupported platform, go fix this.");
            return Ok(vec![]);
        }
    }

    info!("Found {} paths to collect", collection_paths.len());
    Ok(collection_paths)
}

fn mac_folders() -> bool {
    Path::new("/Applications").exists()
        && Path::new("/Users").exists()
        && Path::new("/Library").exists()
}

/// Apply a list of collection entries (patterns, files, directories).
fn apply(
    entries: impl IntoIterator<Item = String>,
    out: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) {
    for entry in entries {
        add_entry(&entry, out, seen);
    }
}

fn exists_push(path: String, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let p = PathBuf::from(&path);
    if p.symlink_metadata().is_ok() && seen.insert(p.clone()) {
        out.push(p);
    } else {
        debug!("skipping absent entry {path}");
    }
}

#[cfg(target_os = "windows")]
fn default_windows(out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, usnjrnl: bool) {
    let system_root = validated_env_base("SystemRoot", "C:\\Windows");
    let program_data = validated_env_base("ProgramData", &format!("{system_root}\\ProgramData"));
    let system_drive = validated_env_base("SystemDrive", "C:");

    let mut entries: Vec<String> = vec![];
    for sub in [
        "Tasks",
        "Prefetch",
        "System32\\sru",
        "System32\\winevt\\Logs",
        "System32\\Tasks",
        "System32\\LogFiles\\W3SVC1",
        "Appcompat\\Programs",
    ] {
        entries.push(format!("{system_root}\\{sub}\\**"));
    }
    entries.push(format!(
        "{program_data}\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\**"
    ));
    entries.push(format!("{system_drive}\\$Recycle.Bin\\**\\$I*"));
    entries.push(format!("{system_drive}\\$Recycle.Bin\\$I*"));

    for sub in [
        "SchedLgU.Txt",
        "inf\\setupapi.dev.log",
        "System32\\drivers\\etc\\hosts",
        "System32\\config\\SAM",
        "System32\\config\\SYSTEM",
        "System32\\config\\SOFTWARE",
        "System32\\config\\SECURITY",
    ] {
        for ext in ["", ".LOG1", ".LOG2"] {
            // SchedLgU/hosts/setupapi have no LOG variants; absent statics are
            // skipped at debug level.
            exists_push(format!("{system_root}\\{sub}{ext}"), out, seen);
        }
    }
    warn!(
        "Raw volume access is used for locked files (SAM, SYSTEM, $MFT, ...): admin rights required"
    );

    // Metafiles exist on every NTFS volume but cannot be opened normally; the
    // raw-access fallback handles them.
    exists_push(format!("{system_drive}\\$LogFile"), out, seen);
    exists_push(format!("{system_drive}\\$MFT"), out, seen);
    if usnjrnl {
        exists_push(format!("{system_drive}\\$Extend\\$UsnJrnl:$J"), out, seen);
    }

    apply(entries, out, seen);

    for user in find_users() {
        if user.profile_path.is_empty() {
            continue;
        }
        let pp = &user.profile_path;
        apply(
            [
                format!("{pp}\\AppData\\Roaming\\Microsoft\\Windows\\Recent\\**"),
                format!("{pp}\\AppData\\Local\\Microsoft\\Windows\\WebCache\\**"),
                format!(
                    "{pp}\\AppData\\Roaming\\Microsoft\\Windows\\Recent\\AutomaticDestinations\\**"
                ),
                format!("{pp}\\AppData\\Roaming\\Mozilla\\Firefox\\Profiles\\**"),
                format!("{pp}\\AppData\\Local\\ConnectedDevicesPlatform\\**"),
                format!("{pp}\\AppData\\Local\\Microsoft\\Windows\\Explorer\\**"),
            ],
            out,
            seen,
        );
        for literal in [
            format!("{pp}\\NTUSER.DAT"),
            format!("{pp}\\NTUSER.DAT.LOG1"),
            format!("{pp}\\NTUSER.DAT.LOG2"),
            format!("{pp}\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat"),
            format!("{pp}\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat.LOG1"),
            format!("{pp}\\AppData\\Local\\Microsoft\\Windows\\UsrClass.dat.LOG2"),
            format!("{pp}\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\History"),
            format!("{pp}\\AppData\\Local\\Microsoft\\Edge\\User Data\\Default\\History"),
            format!(
                "{pp}\\AppData\\Roaming\\Microsoft\\Windows\\PowerShell\\PSReadline\\ConsoleHost_history.txt"
            ),
        ] {
            exists_push(literal, out, seen);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn default_windows(_out: &mut Vec<PathBuf>, _seen: &mut HashSet<PathBuf>, _usnjrnl: bool) {
    // Call site is cfg!-guarded; both branches still compile.
    unreachable!("default_windows called on non-Windows target")
}

/// macOS Chrome/Firefox/preferences used to be matched with disk-wide
/// `**/...` patterns (every plist on the volume). They are anchored to the
/// real library locations instead: system, all user homes, root's home.
fn default_mac(out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let home_roots = ["/Users", "/private/var/root"];
    let mut entries: Vec<String> = vec![];
    for root in home_roots {
        entries.push(format!(
            "{root}/*/Library/Application Support/Google/Chrome/Default/*"
        ));
        entries.push(format!(
            "{root}/*/Library/Application Support/Google/Chrome/Default/Extensions/**"
        ));
        entries.push(format!("{root}/*/Library/Application Support/Firefox/**"));
        entries.push(format!("{root}/*/Library/Preferences/**"));
        entries.push(format!("{root}/*/.*/**history"));
        entries.push(format!(
            "{root}/*/Library/Application Support/com.apple.TCC/TCC.db*"
        ));
    }
    entries.extend([
        "/Library/Application Support/Google/Chrome/Default/*".to_string(),
        "/Library/Preferences/**".to_string(),
        "/System/Library/StartupItems/**".to_string(),
        "/System/Library/LaunchAgents/**".to_string(),
        "/System/Library/LaunchDaemons/**".to_string(),
        "/Library/LaunchAgents/**".to_string(),
        "/Library/LaunchDaemons/**".to_string(),
        "/Library/StartupItems/**".to_string(),
        "/var/log/**".to_string(),
        "/private/var/log/**".to_string(),
        "/private/var/db/diagnostics/**".to_string(),
        "/private/etc/rc.d/**".to_string(),
        "/etc/rc.d/**".to_string(),
        "/.fseventsd/**".to_string(),
    ]);
    apply(entries, out, seen);

    for literal in [
        "/etc/hosts.allow",
        "/etc/hosts.deny",
        "/etc/hosts",
        "/private/etc/hosts.allow",
        "/private/etc/hosts.deny",
        "/private/etc/hosts",
        "/etc/passwd",
        "/etc/group",
        "/private/etc/passwd",
        "/private/etc/group",
    ] {
        exists_push(literal.to_string(), out, seen);
    }
}

fn default_linux(out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>) {
    let mut entries: Vec<String> = vec![];

    // Per-user artifacts are derived from real account homes (/etc/passwd), so
    // nonstandard homes (/export/home, /home2/...) are covered too.
    let mut homes: Vec<String> = vec![];
    for user in find_users() {
        if !user.profile_path.is_empty() {
            homes.push(user.profile_path.clone());
        }
    }
    homes.sort();
    homes.dedup();
    for home in homes {
        for suffix in [
            ".ssh/known_hosts",
            ".ssh/config",
            ".ssh/id_rsa",
            ".ssh/id_ed25519",
            ".ssh/authorized_keys",
            ".bash_history",
            ".zsh_history",
            ".sh_history",
            ".viminfo",
            ".profile",
            ".bashrc",
            ".zshrc",
            ".bash_logout",
            ".zsh_logout",
            ".selected_editor",
            ".wget-hsts",
            ".gitconfig",
        ] {
            entries.push(format!("{home}/{suffix}"));
        }
        entries.push(format!("{home}/.mozilla/firefox/*.default*/**/*.sqlite*"));
        entries.push(format!("{home}/.mozilla/firefox/*.default*/**/*.json"));
        entries.push(format!("{home}/.mozilla/firefox/*.default*/**/*.txt"));
        entries.push(format!("{home}/.mozilla/firefox/*.default*/**/*.db*"));
        for browser_dir in [".config/google-chrome", ".config/chromium"] {
            for name in [
                "History*",
                "Cookies*",
                "Bookmarks*",
                "Last*",
                "Shortcuts*",
                "Top*",
                "Visited*",
                "Preferences*",
                "Login Data*",
                "Web Data*",
            ] {
                entries.push(format!("{home}/{browser_dir}/Default/{name}"));
            }
            entries.push(format!("{home}/{browser_dir}/Default/Extensions/**"));
        }
    }

    entries.extend([
        // Boot / firmware
        "/boot/grub/grub.cfg".to_string(),
        "/boot/grub2/grub.cfg".to_string(),
        "/sys/firmware/acpi/tables/DSDT".to_string(),
        // system identity, auth, scheduling, package + network config
        "/etc/hosts.allow".to_string(),
        "/etc/hosts.deny".to_string(),
        "/etc/hosts".to_string(),
        "/etc/passwd".to_string(),
        "/etc/group".to_string(),
        "/etc/shadow".to_string(),
        "/etc/gshadow".to_string(),
        "/etc/sudoers".to_string(),
        "/etc/crontab".to_string(),
        "/etc/cron.allow".to_string(),
        "/etc/cron.deny".to_string(),
        "/etc/anacrontab".to_string(),
        "/var/spool/anacron/cron.daily".to_string(),
        "/var/spool/anacron/cron.hourly".to_string(),
        "/var/spool/anacron/cron.weekly".to_string(),
        "/var/spool/anacron/cron.monthly".to_string(),
        "/etc/apt/sources.list".to_string(),
        "/etc/apt/trusted.gpg".to_string(),
        "/etc/apt/trustdb.gpg".to_string(),
        "/etc/resolv.conf".to_string(),
        "/etc/fstab".to_string(),
        "/etc/issues".to_string(),
        "/etc/issues.net".to_string(),
        "/etc/insserv.conf".to_string(),
        "/etc/localtime".to_string(),
        "/etc/timezone".to_string(),
        "/etc/pam.conf".to_string(),
        "/etc/rsyslog.conf".to_string(),
        "/etc/xinetd.conf".to_string(),
        "/etc/netgroup".to_string(),
        "/etc/nsswitch.conf".to_string(),
        "/etc/ntp.conf".to_string(),
        "/etc/yum.conf".to_string(),
        "/etc/chrony.conf".to_string(),
        "/etc/logrotate.conf".to_string(),
        "/etc/environment".to_string(),
        "/etc/hostname".to_string(),
        "/etc/host.conf".to_string(),
        "/etc/machine-id".to_string(),
        "/etc/screen-rc".to_string(),
        "/var/log/**".to_string(),
        "/var/spool/at/**".to_string(),
        "/var/spool/cron/**".to_string(),
        "/etc/rc.d/**".to_string(),
        "/etc/cron.daily/**".to_string(),
        "/etc/cron.hourly/**".to_string(),
        "/etc/cron.weekly/**".to_string(),
        "/etc/cron.monthly/**".to_string(),
        "/etc/cron.d/**".to_string(),
        "/etc/modprobe.d/**".to_string(),
        "/etc/modules-load.d/**".to_string(),
        "/etc/*-release".to_string(),
        "/etc/pam.d/**".to_string(),
        "/etc/rsyslog.d/**".to_string(),
        "/etc/yum.repos.d/**".to_string(),
        "/etc/init.d/**".to_string(),
        "/etc/systemd/system/**".to_string(),
        "/etc/default/**".to_string(),
        "/etc/ssh/**".to_string(),
    ]);

    apply(entries, out, seen);
    // root's home is in /etc/passwd already via find_users (home "/root").
}

/// Unix account homes from /etc/passwd (replaces the old stub that returned
/// nothing, which made /home/* hardcoded patterns the only coverage).
#[cfg(unix)]
pub fn find_users() -> Vec<UserProfile> {
    let mut users = vec![];
    let Ok(content) = fs::read_to_string("/etc/passwd") else {
        warn!("cannot read /etc/passwd; user-scoped artifacts will use /home/* only");
        users.push(UserProfile {
            profile_path: "/home/*".to_string(),
            ..Default::default()
        });
        users.push(UserProfile {
            profile_path: "/root".to_string(),
            ..Default::default()
        });
        return users;
    };
    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 6 {
            let home = fields[5];
            if home.starts_with('/') && !home.is_empty() {
                users.push(UserProfile {
                    user_key: fields[0].to_string(),
                    path: line.to_string(),
                    profile_path: home.to_string(),
                    full_profile: 1,
                });
            }
        }
    }
    users
}

#[cfg(target_os = "windows")]
pub fn find_users() -> Vec<UserProfile> {
    let hklm: RegKey = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList");

    let mut user_vec: Vec<UserProfile> = vec![];

    if let Ok(registry_key) = key {
        for user in registry_key.enum_keys().flatten() {
            let path =
                format!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\{user}");
            if let Ok(working_profile) = hklm.open_subkey(&path) {
                let profile_path: String = working_profile
                    .get_value("ProfileImagePath")
                    .unwrap_or_default();
                let full_profile: u32 = working_profile.get_value("FullProfile").unwrap_or(0);
                user_vec.push(UserProfile {
                    user_key: user,
                    path: format!("HKEY_LOCAL_MACHINE\\{path}\\ProfileImagePath"),
                    profile_path,
                    full_profile,
                });
            }
        }
        user_vec
    } else {
        error!("Unable to access profile list registry key.");
        user_vec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tamatoa-paths-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn collect(entries: &[&str]) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut seen = HashSet::new();
        for e in entries {
            add_entry(e, &mut out, &mut seen);
        }
        out.sort();
        out
    }

    #[test]
    fn files_dirs_and_globs_resolve() {
        let dir = fixture("resolve");
        std::fs::write(dir.join("a.log"), b"x").unwrap();
        std::fs::write(dir.join("b.log"), b"x").unwrap();
        std::fs::write(dir.join("skip.txt"), b"x").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/c.log"), b"x").unwrap();

        let base = dir.to_str().unwrap();
        let direct = collect(&[&format!("{base}/a.log")]);
        assert_eq!(direct, vec![dir.join("a.log")]);

        let g = collect(&[&format!("{base}/*.log")]);
        assert_eq!(g, vec![dir.join("a.log"), dir.join("b.log")]);

        // Directory argument expands recursively.
        let walked = collect(&[base]);
        assert!(walked.contains(&dir.join("a.log")));
        assert!(walked.contains(&dir.join("sub/c.log")));
        assert!(!walked.contains(&dir)); // the directory itself is not an entry

        // In the glob crate `**` spans zero or more directories, so
        // `base/**/*.log` is the any-depth superset (files in base included).
        // Artifact patterns rely on exactly this breadth.
        let dd = collect(&[&format!("{base}/**/*.log")]);
        assert_eq!(
            dd,
            vec![dir.join("a.log"), dir.join("b.log"), dir.join("sub/c.log")]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walk_skips_symlinked_directories() {
        let dir = fixture("walksym");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/f.txt"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();

        let files = walk_tree(dir.clone());
        assert!(files.contains(&dir.join("real/f.txt")));
        assert!(
            !files.iter().any(|p| p.starts_with(dir.join("link"))),
            "symlinked dir followed: {files:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn globs_are_case_sensitive_on_unix() {
        let dir = fixture("case");
        std::fs::write(dir.join("SECRET.log"), b"x").unwrap();
        std::fs::write(dir.join("secret.LOG"), b"x").unwrap();
        let base = dir.to_str().unwrap();
        let g = collect(&[&format!("{base}/*.log")]);
        assert_eq!(g, vec![dir.join("SECRET.log")]);
        let g2 = collect(&[&format!("{base}/*.LOG")]);
        assert_eq!(g2, vec![dir.join("secret.LOG")]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn duplicates_are_deduplicated() {
        let dir = fixture("dedupe");
        std::fs::write(dir.join("x.log"), b"x").unwrap();
        let base = dir.to_str().unwrap();
        let g = collect(&[
            &format!("{base}/x.log"),
            &format!("{base}/*.log"),
            &format!("{base}/**/*.log"),
            base,
        ]);
        assert_eq!(g, vec![dir.join("x.log")]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_parsing_skips_comments_expands_env() {
        let dir = fixture("cfg");
        std::env::set_var("TAMATOA_TEST_HOME", "/home/evidencedir");
        std::fs::write(
            dir.join("cfg.txt"),
            "# comment line\n\n/var/log/app.log\n$TAMATOA_TEST_HOME/app.log\n${TAMATOA_TEST_HOME}/sub\n%TAMATOA_TEST_HOME%/winstyle\n",
        )
        .unwrap();
        let entries = parse_config(&dir.join("cfg.txt")).unwrap();
        assert_eq!(
            entries,
            vec![
                "/var/log/app.log",
                "/home/evidencedir/app.log",
                "/home/evidencedir/sub",
                "/home/evidencedir/winstyle"
            ]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_files_are_kept_for_manifest_reporting() {
        // A concrete path whose parent exists but which is absent still shows
        // up, so the manifest records the failure rather than hiding it.
        let dir = fixture("missing");
        let g = collect(&[dir.join("absent.log").to_str().unwrap()]);
        assert_eq!(g, vec![dir.join("absent.log")]);
        // Deep nonsense (parent chain missing) is dropped quietly.
        let g2 = collect(&["/no/such/dir/at/all/file.log"]);
        assert!(g2.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn prunes_pseudo_filesystems() {
        assert!(pruned(Path::new("/proc")));
        assert!(pruned(Path::new("/sys")));
        assert!(!pruned(Path::new("/var/log")));
        // /proc actually exists on the test machine; walking it would list
        // volatile nonsense. Prove it does not appear.
        let files = walk_tree(PathBuf::from("/"));
        assert!(
            !files.iter().any(|f| f.starts_with("/proc/")),
            "walk descended into /proc"
        );
    }

    #[test]
    fn expand_env_marks_unknown_vars_unmatchable() {
        let s = expand_env("/etc/$TOTALLY_UNSET_TAMATOA/file");
        assert!(s.contains("%UNSET-TOTALLY_UNSET_TAMATOA%"), "{s}");
        assert_eq!(expand_env("plain/path"), "plain/path");
        std::env::set_var("TAMATOA_T2", "X");
        assert_eq!(expand_env("%TAMATOA_T2%/y"), "X/y");
        assert_eq!(expand_env("${TAMATOA_T2}/y"), "X/y");
        assert_eq!(expand_env("$TAMATOA_T2/y"), "X/y");
    }
}
