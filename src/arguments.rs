use clap::{ArgAction, Parser};
use std::path::PathBuf;

/// CyLR-compatible single-dash multi-character flags, rewritten to their clap
/// long equivalents before parsing (`clap` treats `-of` as a short flag cluster).
const COMPAT_FLAGS: [(&str, &str); 4] = [
    ("-of", "--output-file"),
    ("-od", "--output-dir"),
    ("-hf", "--hash-files"),
    ("-zl", "--zip-level"),
];

/// Normalize process arguments: CyLR flag spellings, the Windows `/?` help
/// convention, and `--` end-of-options (after which everything is positional).
pub fn normalize_args<I, S>(raw: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut out: Vec<String> = Vec::new();
    let mut literal = false;
    for arg in raw {
        let arg: String = arg.into();
        if literal || arg == "--" {
            literal |= arg == "--";
            out.push(arg);
            continue;
        }
        if arg == "/?" {
            out.push("--help".to_string());
            continue;
        }
        match COMPAT_FLAGS
            .iter()
            .find(|(short, _)| *short == arg.as_str())
        {
            Some((_, long)) => out.push((*long).to_string()),
            None => out.push(arg),
        }
    }
    out
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "tamatoa",
    about = "Forensic evidence collection tool.\n\n\
        Collects files, directories and glob patterns into a ZIP archive with a\n\
        SHA-256 manifest. Exit codes: 0 = complete, 1 = partial (some artifacts\n\
        failed), 2 = fatal error / bad usage.",
    version = crate::version::VERSION_STRING.as_str(),
    max_term_width = 100,
)]
pub struct Cli {
    /// Only log errors.
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// Log level: trace | debug | info | warn | error.
    #[arg(short = 'v', long = "verbosity", default_value = "info",
          value_parser = ["trace", "debug", "info", "warn", "error"])]
    pub verbosity: String,

    /// Config file with one path/glob per line; replaces the default artifact set.
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Config file applied in addition to the default artifact set.
    #[arg(short = 'd', long = "config-defaults", value_name = "FILE")]
    pub config_with_defaults: Option<PathBuf>,

    /// Directory for the archive; must already exist.
    #[arg(long = "output-dir", value_name = "DIR")]
    pub output_dir: Option<PathBuf>,

    /// Archive file name. Default: <host>-<UTC timestamp>.zip
    #[arg(long = "output-file", value_name = "NAME")]
    pub output_file: Option<String>,

    /// Overwrite an existing archive instead of refusing.
    #[arg(long = "force")]
    pub force: bool,

    /// Include $UsnJrnl streams in the Windows collection.
    #[arg(long = "usnjrnl")]
    pub usnjrnl: bool,

    /// Deprecated no-op: every artifact is SHA-256 hashed while it is archived.
    #[arg(long = "hash-files", action = ArgAction::SetTrue)]
    pub hash_files: bool,

    /// Deflate compression level 0-9. 0 stores entries without compression.
    #[arg(
        long = "zip-level",
        default_value_t = 6,
        value_name = "LEVEL",
        value_parser = clap::value_parser!(u32).range(0..=9)
    )]
    pub zip_level: u32,

    /// Per-artifact byte budget; oversized sources are skipped, not streamed
    /// forever. Default 128 GiB; 0 disables.
    #[arg(long = "max-file-bytes", value_name = "BYTES", default_value_t = 128u64 << 30)]
    pub max_file_bytes: u64,

    /// Total payload byte budget for the whole run. Default 1 TiB; 0 disables.
    #[arg(long = "max-total-bytes", value_name = "BYTES", default_value_t = 1u64 << 40)]
    pub max_total_bytes: u64,

    /// Emit a machine-readable JSON summary as the only stdout line.
    #[arg(long = "json")]
    pub json: bool,

    /// Additional files, directories or glob patterns to collect.
    pub paths: Vec<PathBuf>,
}

impl Cli {
    pub fn parse_cli() -> Self {
        Self::parse_from(normalize_args(std::env::args()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(args: &[&str]) -> Vec<String> {
        normalize_args(args.iter().map(|s| s.to_string()))
    }

    /// Parse exactly the way production does: normalize, then clap.
    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(norm(args))
    }

    #[test]
    fn cyrl_flags_are_rewritten_to_longs() {
        assert_eq!(
            norm(&["tamatoa", "-of", "a.zip", "-od", "/tmp", "-hf", "-zl", "0"]),
            vec![
                "tamatoa",
                "--output-file",
                "a.zip",
                "--output-dir",
                "/tmp",
                "--hash-files",
                "--zip-level",
                "0"
            ]
        );
    }

    #[test]
    fn question_mark_is_help() {
        assert_eq!(norm(&["tamatoa", "/?"]), vec!["tamatoa", "--help"]);
    }

    #[test]
    fn double_dash_freezes_everything_after_it() {
        assert_eq!(
            norm(&["tamatoa", "--", "-of", "not_a_flag.txt"]),
            vec!["tamatoa", "--", "-of", "not_a_flag.txt"]
        );
    }

    #[test]
    fn unknown_flags_pass_through_for_clap_to_reject() {
        assert!(Cli::try_parse_from(["tamatoa", "-zz"]).is_err());
    }

    #[test]
    fn values_following_flags_are_not_collected_as_paths() {
        let cli = parse(&[
            "tamatoa", "-of", "out.zip", "-od", "/tmp", "-zl", "3", "afile", "another",
        ])
        .unwrap();
        assert_eq!(cli.output_file.as_deref(), Some("out.zip"));
        assert_eq!(cli.zip_level, 3);
        assert_eq!(
            cli.paths,
            vec![PathBuf::from("afile"), PathBuf::from("another")]
        );
    }

    #[test]
    fn every_trailing_argument_is_kept() {
        let cli = parse(&["tamatoa", "a", "b", "c"]).unwrap();
        assert_eq!(cli.paths.len(), 3);
    }

    #[test]
    fn zip_level_range_is_enforced() {
        assert!(parse(&["tamatoa", "-zl", "10"]).is_err());
        assert!(parse(&["tamatoa", "-zl", "0"]).is_ok());
    }

    #[test]
    fn hash_files_compat_flag_parses() {
        let cli = parse(&["tamatoa", "-hf", "x"]).unwrap();
        assert!(cli.hash_files);
    }

    #[test]
    fn default_verbosity_is_info() {
        let cli = parse(&["tamatoa", "x"]).unwrap();
        assert_eq!(cli.verbosity, "info");
        assert!(!cli.quiet);
    }
}
