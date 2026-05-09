use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "flowcase_upload_server",
    version,
    about = "Chunked Dropzone-compatible upload server."
)]
pub struct Cli {
    /// Enable TLS with an in-memory self-signed cert (matches the legacy
    /// Flask `ssl_context="adhoc"` flag).
    #[arg(long, default_value_t = false)]
    pub ssl: bool,

    /// Required HTTP Basic auth token in `user:pass` form. Without it,
    /// every request returns 403 — matches the legacy Python.
    #[arg(long = "auth-token")]
    pub auth_token: String,

    /// Listen port.
    #[arg(long, default_value_t = 4902)]
    pub port: u16,

    /// Directory to write uploaded files to.
    #[arg(long = "upload-dir", default_value_os_t = default_upload_dir())]
    pub upload_dir: PathBuf,
}

fn default_upload_dir() -> PathBuf {
    // Mirrors os.path.join(os.getenv("HOME"), "Uploads") from the legacy
    // Python. If $HOME is unset the path falls back to "./Uploads"; on
    // a real droplet $HOME is always set.
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join("Uploads")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_invocation() {
        let cli = Cli::try_parse_from([
            "flowcase_upload_server",
            "--port",
            "4902",
            "--auth-token",
            "foo",
            "--upload-dir",
            "/tmp",
        ])
        .expect("expected the canonical invocation to parse");

        assert!(!cli.ssl);
        assert_eq!(cli.port, 4902);
        assert_eq!(cli.auth_token, "foo");
        assert_eq!(cli.upload_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn ssl_flag_is_a_bool() {
        let cli = Cli::try_parse_from(["flowcase_upload_server", "--ssl", "--auth-token", "u:p"])
            .expect("ssl + auth-token alone should parse, defaults fill the rest");

        assert!(cli.ssl);
        assert_eq!(cli.port, 4902);
        assert_eq!(cli.auth_token, "u:p");
    }

    #[test]
    fn auth_token_is_required() {
        let result = Cli::try_parse_from(["flowcase_upload_server"]);
        assert!(result.is_err(), "auth-token should be required");
    }
}
