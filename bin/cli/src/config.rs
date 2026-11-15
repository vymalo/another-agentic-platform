//! The file `aapctl` keeps between runs: the server's address and the access token (§60b).
//!
//! * **Where**: `--config`, else `$XDG_CONFIG_HOME/aapctl/config.yaml`, else `$HOME/.config/aapctl/config.yaml`. A directory made for
//!   it is `0700`.
//! * **Written whole or not at all**: to a temporary file made with `create_new` and mode `0600`, flushed to disk, then renamed over
//!   the old file, so a reader sees the old file or the new one and a crash leaves no half-written token.
//! * **A file that group or others can read is refused** (exit 2), as `ssh` refuses a private key: a token is a credential, and a
//!   file that leaks it is not used until its owner has fixed the mode.
//! * **No error here holds the file's text**: a parse error of a YAML library can quote a value, and the value is the token.
//! * There is no keyring: none is in the lockfile (P-031). The token is in this file and in `AAPCTL_TOKEN`, never in an argument.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The environment `aapctl` reads for the network commands, and only for them: `validate` and `render` read none.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `AAPCTL_TOKEN`: an access token that takes precedence over the file and is never written to it.
    pub token: Option<String>,
    /// `XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<OsString>,
    /// `HOME`.
    pub home: Option<OsString>,
}

impl Env {
    /// The process's own.
    pub fn process() -> Self {
        Self {
            token: std::env::var("AAPCTL_TOKEN")
                .ok()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty()),
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            home: std::env::var_os("HOME"),
        }
    }
}

/// What the file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// The Platform API's address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// The access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// When the access token expires (seconds since the epoch), if it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// Where the file is, when `--config` is not given. `None` when the environment names neither a config home nor a home.
pub fn default_path(env: &Env) -> Option<PathBuf> {
    let absolute = |p: &OsString| !p.is_empty() && Path::new(p).is_absolute();
    match (&env.xdg_config_home, &env.home) {
        (Some(x), _) if absolute(x) => Some(Path::new(x).join("aapctl").join("config.yaml")),
        (_, Some(h)) if absolute(h) => Some(
            Path::new(h)
                .join(".config")
                .join("aapctl")
                .join("config.yaml"),
        ),
        _ => None,
    }
}

/// Read the file. A file that is not there is an empty configuration.
///
/// # Errors
///
/// A sentence for exit 2: the file cannot be read, is readable by group or others, or is not a configuration. It never holds the
/// file's text.
pub fn load(path: &Path) -> Result<Config, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(format!("{}: {}", path.display(), e.kind())),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "{} is readable by group or others (mode {mode:04o}) and holds a credential: run `chmod 600 {}` and try again",
                path.display(),
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
    serde_yaml::from_str(&text).map_err(|_| {
        format!(
            "{} is not an aapctl configuration (a YAML mapping of server, accessToken and expiresAt)",
            path.display()
        )
    })
}

/// Write the file whole or not at all, with mode `0600`, in a directory that is `0700` if it had to be made.
///
/// # Errors
///
/// A sentence for exit 2. It never holds the token.
pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    let fail = |what: &str, e: &std::io::Error| format!("{}: {what}: {}", path.display(), e.kind());
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        make_private_dir(dir).map_err(|e| fail("cannot make the directory", &e))?;
    }
    let text = serde_yaml::to_string(config)
        .map_err(|_| "the configuration cannot be written".to_owned())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temporary = dir.join(format!(
        ".{}.tmp-{}-{stamp}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config"),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(fail("cannot write the file", &e));
    }
    Ok(())
}

fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}
