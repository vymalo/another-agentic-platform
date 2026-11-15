//! The file `aapctl` keeps between runs: the server's address and the access token (§60b).
//!
//! * **Where**: `--config`, else `$XDG_CONFIG_HOME/aapctl/config.yaml`, else `$HOME/.config/aapctl/config.yaml`. A directory made for
//!   it is `0700`.
//! * **Written whole or not at all**: to a temporary file made with `create_new` and mode `0600`, flushed to disk, then renamed over
//!   the old file, so a reader sees the old file or the new one and a crash leaves no half-written token.
//! * **A file that group or others can read is refused** (exit 2), as `ssh` refuses a private key: a token is a credential, and a
//!   file that leaks it is not used until its owner has fixed the mode.
//! * **It is read safely**: opened once without following a symbolic link at its last component (`O_NOFOLLOW`), then the *open
//!   descriptor* is checked (`fstat`: a regular file, owned by the effective user, no group or other permission bit) and read, so
//!   the file that was checked is the file that was read. What is **not** checked: the modes of the directories above it (an
//!   existing directory is used as it is; one made for the file is `0700`).
//! * **The token and the server go together**: the token in the file belongs to the server in the file, and is never sent to
//!   another one ([`crate::remote`] checks it).
//! * **No error here holds the file's text**: a parse error of a YAML library can quote a value, and the value is the token.
//! * There is no keyring: none is in the lockfile (P-031). The token is in this file and in `AAPCTL_TOKEN`, never in an argument.

use std::ffi::OsString;
use std::io::{Read, Write};
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
    let mut file = match open_private(path) {
        Ok(f) => f,
        Err(Refused::Missing) => return Ok(Config::default()),
        Err(Refused::Because(why)) => return Err(why),
    };
    // The whole file is a few hundred bytes: one that is far larger is not ours.
    let mut text = String::new();
    Read::take(&mut file, MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: {}", path.display(), e.kind()))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "{} is far larger than an aapctl configuration",
            path.display()
        ));
    }
    serde_yaml::from_str(&text).map_err(|_| {
        format!(
            "{} is not an aapctl configuration (a YAML mapping of server, accessToken and expiresAt)",
            path.display()
        )
    })
}

/// The most a configuration file may hold.
const MAX_FILE_BYTES: u64 = 64 * 1024;

enum Refused {
    Missing,
    Because(String),
}

/// Open the file for reading and check **the open descriptor**: no symbolic link at the last component, a regular file, owned by the
/// effective user, no permission bit for group or others.
#[cfg(unix)]
fn open_private(path: &Path) -> Result<std::fs::File, Refused> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let shown = path.display();
    let file = std::fs::OpenOptions::new()
        .read(true)
        // `O_NONBLOCK`: `open(2)` of a FIFO with no writer blocks until there is one, before the descriptor can be checked. A regular
        // file is read the same either way.
        .custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK)
                .bits()
                .cast_signed(),
        )
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Refused::Missing,
            _ if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
                Refused::Because(format!(
                    "{shown} is a symbolic link, and a file that holds a credential is not followed through one: give --config the real path"
                ))
            }
            kind => Refused::Because(format!("{shown}: {kind}")),
        })?;
    let meta = file
        .metadata()
        .map_err(|e| Refused::Because(format!("{shown}: {}", e.kind())))?;
    if !meta.is_file() {
        return Err(Refused::Because(format!("{shown} is not a regular file")));
    }
    let owner = rustix::process::geteuid().as_raw();
    if meta.uid() != owner {
        return Err(Refused::Because(format!(
            "{shown} is not owned by you (uid {}) and holds a credential: it is not used",
            meta.uid()
        )));
    }
    let mode = meta.mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(Refused::Because(format!(
            "{shown} is readable by group or others (mode {mode:04o}) and holds a credential: run `chmod 600 {shown}` and try again"
        )));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> Result<std::fs::File, Refused> {
    std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Refused::Missing,
        kind => Refused::Because(format!("{}: {kind}", path.display())),
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
    write_through(path, &temporary, text.as_bytes()).map_err(|e| fail("cannot write the file", &e))
}

/// Write `bytes` to `temporary`, which must not exist (`O_EXCL`: a symbolic link someone left at that name is not followed, and a
/// file there is not truncated), with mode `0600`, flush it, and rename it over `path`. **A temporary file made is removed when
/// anything after its creation fails.**
fn write_through(path: &Path, temporary: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
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

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aapctl-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_symbolic_link_at_the_temporary_name_is_not_followed_and_the_victim_is_untouched() {
        let d = dir("exclusive");
        let victim = d.join("victim");
        std::fs::write(&victim, "precious").unwrap();
        let temporary = d.join(".config.yaml.tmp-1-2");
        symlink(&victim, &temporary).unwrap();
        let target = d.join("config.yaml");
        let error = write_through(&target, &temporary, b"accessToken: secret\n").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
        assert!(!target.exists());
        // What was there is not ours to remove either.
        assert!(temporary.symlink_metadata().is_ok());
    }

    #[test]
    fn a_temporary_file_is_removed_when_the_write_fails_after_its_creation() {
        let d = dir("cleanup");
        // The rename cannot succeed: the target is a directory with something in it.
        let target = d.join("config.yaml");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("x"), "x").unwrap();
        let temporary = d.join(".config.yaml.tmp-1-3");
        assert!(write_through(&target, &temporary, b"a: b\n").is_err());
        assert!(
            temporary.symlink_metadata().is_err(),
            "the temporary file was left behind"
        );
    }

    #[test]
    fn a_file_is_read_through_its_descriptor_not_through_a_link_and_not_when_it_is_open_to_others()
    {
        let d = dir("safe-read");
        let real = d.join("real.yaml");
        std::fs::write(&real, "server: https://x.example\naccessToken: t\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&real).is_ok());
        // A link to a good file is refused: the last component is not followed.
        let link = d.join("link.yaml");
        symlink(&real, &link).unwrap();
        let error = load(&link).unwrap_err();
        assert!(error.contains("symbolic link"), "{error}");
        // Not a regular file.
        let error = load(&d).unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
        // Group or other bits, any of them.
        for mode in [0o640, 0o604, 0o660, 0o644] {
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = load(&real).unwrap_err();
            assert!(
                error.contains("readable by group or others"),
                "{mode:o}: {error}"
            );
        }
        // Gone: an empty configuration.
        assert_eq!(load(&d.join("nope.yaml")).unwrap(), Config::default());
    }

    #[test]
    fn a_file_larger_than_a_configuration_is_refused() {
        let d = dir("big");
        let big = d.join("big.yaml");
        std::fs::write(&big, "x".repeat(70_000)).unwrap();
        std::fs::set_permissions(&big, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&big).unwrap_err().contains("far larger"));
    }
}
