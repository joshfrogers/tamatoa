use std::sync::LazyLock;

pub fn get_version() -> &'static str {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    VERSION
}

/// Full version including the build-time git hash, e.g. `0.3.0 (abc1234)`.
pub static VERSION_STRING: LazyLock<String> =
    LazyLock::new(|| format!("{} ({})", get_version(), env!("TAMATOA_GIT_HASH")));
