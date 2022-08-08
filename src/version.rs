pub fn get_version() -> &'static str {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    VERSION
}
