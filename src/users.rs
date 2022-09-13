use std::process::{Command, Stdio};

#[cfg(target_os = "windows")]
use windows_drives;
#[cfg(target_os = "windows")]
use winreg::enums::*;
#[cfg(target_os = "windows")]
use winreg::RegKey;
use crate::errors::ErrCode;

#[derive(Default, Debug)]
pub struct UserProfile {
    pub user_key: String,
    pub path: String,
    pub profile_path: String,
    pub full_profile: u32,
    password: String,
    uid: u8,
    gid: u8,
    comment: String,
    shell:String,
}

impl UserProfile {
    pub fn new() -> UserProfile {
        UserProfile {
            user_key: "".to_string(),
            path: "".to_string(),
            profile_path: "".to_string(),
            full_profile: 0,
            password: "".to_string(),
            uid: 0,
            gid: 0,
            comment: "".to_string(),
            shell: "".to_string()
        }
    }
}


#[cfg(target_os="macos")]
pub fn find_users() -> Result<Vec<UserProfile>, ErrCode> {
    // So - as there are no bindings for dscl on mac, nor crates for interacting with macos directory
    // we can either:
    //  - write our own lib
    //  - execute the command and read the output.
    //  - read /etc/passwd. Which doesnt work cause it's a mac.
    Ok(vec![])
}

#[cfg(target_os="linux")]
pub fn find_users() -> Result<Vec<UserProfile>, ErrCode> {
    let passwd = std::fs::read_to_string("/etc/passwd")?;
    let mut passwd_lines: Vec<&str> = passwd.split("\n").filter(|line| !line.starts_with("#") && !line.starts_with("_")).collect();
    let mut users: Vec<UserProfile> = vec![];
    for line in passwd_lines {
        let user_line: Vec<&str> = line.split(":").collect();
        if user_line.len() > 1 {
            let mut line = user_line.into_iter();
            users.push(UserProfile {
                user_key: line.next().unwrap().parse().unwrap(),
                password: line.next().unwrap().parse().unwrap(),
                uid: line.next().unwrap().parse().unwrap_or(0),
                gid: line.next().unwrap().parse().unwrap_or(0),
                comment: line.next().unwrap().parse().unwrap(),
                path: "".to_string(),
                profile_path: line.next().unwrap().parse().unwrap(),
                full_profile: 0,
                shell: line.next().unwrap().parse().unwrap()
            }
            )
        }
    }
    Ok(users)
}

#[cfg(target_os="windows")]
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