use std::env;

pub fn is_unix_like() -> bool {
    // std::env::consts::OS returns:
    // linux
    // macos
    // ios
    // freebsd
    // dragonfly
    // netbsd
    // openbsd
    // solaris
    // android
    // windows
    // This may be a naive method of checking.
    let os: &str = env::consts::OS;
    return if (os == "linux") || (os == "macos") {
        true
    } else {
        false
    };
}

pub fn supports_raw_access() -> bool {
    return !is_unix_like();
}

// pub fn is_input_redirected() -> bool {
//     //     check if debug build, return false if Debug else
//     //     check system is input redirected?
//     //     Unsure how we can check this - returning false for now, will come back an fix it
//     false
// }
