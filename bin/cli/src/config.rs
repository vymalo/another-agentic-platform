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
//! * **What [`save`] accepts, [`load`] reads**: a file longer than [`MAX_FILE_BYTES`] is refused by `load`, so `save` refuses to write
//!   one (and the tokens that go in are bounded: 16 KiB each, a few scopes, so a refresh cannot make a file the next command
//!   refuses). A file that is ours (owner, mode, link and type pass) but too large or not a configuration is **replaceable**:
//!   [`load_replaceable`] says so, and `login` writes over it and `logout` removes it; every other command still refuses it.
//! * **One writer at a time**: [`lock`] is an advisory `flock` on `<file>.lock` (mode `0600`, never removed, opened without following
//!   a link) around a read-compare-write, so a refresh that races a logout or another refresh finds what the other wrote.
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
    /// The OpenID Connect issuer the browser sign-in used. **The refresh token is bound to it**: it is sent to the token endpoint
    /// discovered from this issuer and to no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// The public client the sign-in used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The scopes the sign-in asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// The refresh token, if the issuer gave one. A secret like the access token: the file is `0600` and it is printed nowhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// How the access token was obtained: [`SIGNED_IN`] or [`PASTED`]. A file written before this field existed has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_source: Option<String>,
}

/// `tokenSource` of a token that `aapctl login` got from the issuer.
pub const SIGNED_IN: &str = "signed-in";

/// `tokenSource` of a token that was piped to `aapctl login --token-stdin`.
pub const PASTED: &str = "pasted";

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
    read(path).map_err(|why| match why {
        Unreadable::Refused(why) => why,
        Unreadable::Unusable(why) => {
            format!("{why}: `aapctl logout` removes it and `aapctl login` writes a new one")
        }
    })
}

/// [`load`] for the two commands that may replace a file: `Ok((config, Some(why)))` for a file that is ours (the owner, the mode, the
/// link and the type were checked) but is too large or is not a configuration, with an empty configuration in its place; the
/// sentence says what was wrong with it. A file that fails a check is still an error.
///
/// # Errors
///
/// As [`load`], for everything but an unusable content.
pub fn load_replaceable(path: &Path) -> Result<(Config, Option<String>), String> {
    match read(path) {
        Ok(config) => Ok((config, None)),
        Err(Unreadable::Unusable(why)) => Ok((Config::default(), Some(why))),
        Err(Unreadable::Refused(why)) => Err(why),
    }
}

/// Why a file was not read: a check failed (the file is not ours to touch), or it is ours and its content is no use.
enum Unreadable {
    Refused(String),
    Unusable(String),
}

fn read(path: &Path) -> Result<Config, Unreadable> {
    let mut file = match open_private(path) {
        Ok(f) => f,
        Err(Refused::Missing) => return Ok(Config::default()),
        Err(Refused::Because(why)) => return Err(Unreadable::Refused(why)),
    };
    // The whole file is a few hundred bytes: one that is far larger is not ours.
    let mut text = String::new();
    Read::take(&mut file, MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| {
            // A file that is not UTF-8 is not a configuration; any other failure is the disk's.
            if e.kind() == std::io::ErrorKind::InvalidData {
                Unreadable::Unusable(format!("{} is not an aapctl configuration", path.display()))
            } else {
                Unreadable::Refused(format!("{}: {}", path.display(), e.kind()))
            }
        })?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(Unreadable::Unusable(format!(
            "{} is far larger than an aapctl configuration",
            path.display()
        )));
    }
    serde_yaml::from_str(&text).map_err(|_| {
        Unreadable::Unusable(format!(
            "{} is not an aapctl configuration (a YAML mapping: server, accessToken, expiresAt, issuer, clientId, scopes, refreshToken, tokenSource)",
            path.display()
        ))
    })
}

/// The most a configuration file may hold, and so the most [`save`] writes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;

/// Remove a configuration file that [`load_replaceable`] found unusable. A missing file is not an error.
///
/// # Errors
///
/// A sentence for exit 2.
pub fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "{}: cannot remove it: {}",
            path.display(),
            e.kind()
        )),
    }
}

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

/// Write `new` only if the file still holds `expected`, the configuration this process read: **two `aapctl` processes that refresh at
/// once must not destroy a newer token** (an issuer that rotates refresh tokens gives the second a token the first already used). `Ok(false)`:
/// the file changed since it was read and was left as it is. The check and the rename are not one step, so a write that lands between
/// them is still lost: the limit is documented (`bin/cli/README.md`), and the loss is a sign-in the person repeats.
///
/// # Errors
///
/// As [`load`] and [`save`].
pub fn save_if_unchanged(path: &Path, expected: &Config, new: &Config) -> Result<bool, String> {
    if load(path)? != *expected {
        return Ok(false);
    }
    save(path, new)?;
    Ok(true)
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
    // What is written must be what `load` reads: a file it would refuse is a lockout until it is deleted by hand.
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(format!(
            "{}: the configuration would be larger than {MAX_FILE_BYTES} bytes, which aapctl does not read back: nothing was written",
            path.display()
        ));
    }
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

/// How long [`lock`] waits for another `aapctl` to finish.
pub const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Why the lock was not taken.
#[derive(Debug)]
pub enum LockError {
    /// Another process held it for the whole wait.
    Busy,
    /// The lock file cannot be used (a link, not ours, not writable): a sentence for exit 2.
    Failed(String),
}

/// The lock on a configuration file: held until it is dropped. On a platform with no `flock` it holds nothing.
#[derive(Debug)]
pub struct Lock {
    _file: Option<std::fs::File>,
}

/// The lock file of a configuration file: `<file>.lock`, in the same directory.
pub fn lock_path(config: &Path) -> PathBuf {
    let mut name = config
        .file_name()
        .map_or_else(|| OsString::from("config"), OsString::from);
    name.push(".lock");
    config.with_file_name(name)
}

/// Take the advisory lock around a read-compare-write of `config`, waiting at most [`LOCK_WAIT`]. The lock file is `<config>.lock`,
/// mode `0600`, opened without following a symbolic link at its last component and checked on the open descriptor (a regular file,
/// ours), and **never removed** (removing it would let two processes lock two files). Not a security boundary: any process of the
/// same user can lock it too, and the only harm is a wait.
///
/// # Errors
///
/// [`LockError::Busy`] after the wait; [`LockError::Failed`] for a lock file that cannot be used.
pub fn lock(config: &Path) -> Result<Lock, LockError> {
    lock_for(config, LOCK_WAIT)
}

/// [`lock`] with its own wait.
///
/// # Errors
///
/// As [`lock`].
#[cfg(unix)]
pub fn lock_for(config: &Path, wait: std::time::Duration) -> Result<Lock, LockError> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let path = lock_path(config);
    let shown = path.display();
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !dir.exists() {
        make_private_dir(dir).map_err(|e| {
            LockError::Failed(format!(
                "{}: cannot make the directory: {}",
                dir.display(),
                e.kind()
            ))
        })?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
        .open(&path)
        .map_err(|e| {
            if e.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
                LockError::Failed(format!(
                    "{shown} is a symbolic link, which the lock is not taken through: remove it"
                ))
            } else {
                LockError::Failed(format!("{shown}: {}", e.kind()))
            }
        })?;
    let meta = file
        .metadata()
        .map_err(|e| LockError::Failed(format!("{shown}: {}", e.kind())))?;
    if !meta.is_file() || meta.uid() != rustix::process::geteuid().as_raw() {
        return Err(LockError::Failed(format!(
            "{shown} is not a regular file of yours: the lock is not taken on it"
        )));
    }
    let started = std::time::Instant::now();
    loop {
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Ok(Lock { _file: Some(file) }),
            Err(e) if e == rustix::io::Errno::WOULDBLOCK || e == rustix::io::Errno::INTR => {
                if started.elapsed() >= wait {
                    return Err(LockError::Busy);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => {
                return Err(LockError::Failed(format!(
                    "{shown}: cannot lock it: {}",
                    std::io::Error::from(e).kind()
                )));
            }
        }
    }
}

/// [`lock`] where there is no `flock`: nothing is held, and [`save_if_unchanged`] alone keeps a newer token from being destroyed.
///
/// # Errors
///
/// Never.
#[cfg(not(unix))]
pub fn lock_for(_config: &Path, _wait: std::time::Duration) -> Result<Lock, LockError> {
    Ok(Lock { _file: None })
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
    fn a_write_that_would_destroy_a_newer_token_is_not_made() {
        let d = dir("compare");
        let path = d.join("config.yaml");
        let read = Config {
            server: Some("https://x.example".into()),
            access_token: Some("old".into()),
            refresh_token: Some("r-old".into()),
            ..Config::default()
        };
        save(&path, &read).unwrap();
        let mine = Config {
            access_token: Some("mine".into()),
            refresh_token: Some("r-mine".into()),
            ..read.clone()
        };
        // Another process wrote a newer token between this one's read and its write.
        let theirs = Config {
            access_token: Some("theirs".into()),
            refresh_token: Some("r-theirs".into()),
            ..read.clone()
        };
        save(&path, &theirs).unwrap();
        assert!(!save_if_unchanged(&path, &read, &mine).unwrap());
        assert_eq!(load(&path).unwrap(), theirs);
        // Nobody wrote: the write is made.
        assert!(save_if_unchanged(&path, &theirs, &mine).unwrap());
        assert_eq!(load(&path).unwrap(), mine);
    }

    #[test]
    fn a_file_larger_than_a_configuration_is_refused() {
        let d = dir("big");
        let big = d.join("big.yaml");
        std::fs::write(&big, "x".repeat(70_000)).unwrap();
        std::fs::set_permissions(&big, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = load(&big).unwrap_err();
        assert!(
            error.contains("far larger") && error.contains("aapctl logout"),
            "{error}"
        );
        // It is ours, so a login or a logout may replace it; one that fails a check is not.
        let (config, why) = load_replaceable(&big).unwrap();
        assert_eq!(config, Config::default());
        assert!(why.unwrap().contains("far larger"));
        std::fs::set_permissions(&big, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            load_replaceable(&big)
                .unwrap_err()
                .contains("readable by group or others")
        );
        // Not a mapping: also replaceable. A good file and a missing one are not "unusable".
        let odd = d.join("odd.yaml");
        std::fs::write(&odd, "- a\n- b\n").unwrap();
        std::fs::set_permissions(&odd, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            load_replaceable(&odd)
                .unwrap()
                .1
                .unwrap()
                .contains("not an aapctl configuration")
        );
        assert_eq!(
            load_replaceable(&d.join("none.yaml")).unwrap(),
            (Config::default(), None)
        );
    }

    #[test]
    fn whatever_save_accepts_load_reads() {
        let d = dir("invariant");
        let path = d.join("config.yaml");
        // The most the inputs allow: two tokens of 16 KiB, 32 scopes of 256 characters, long addresses.
        let largest = Config {
            server: Some(format!("https://{}", "s".repeat(2000))),
            access_token: Some("a".repeat(16 * 1024)),
            expires_at: Some(i64::MAX),
            issuer: Some(format!("https://{}", "i".repeat(2000))),
            client_id: Some("c".repeat(256)),
            scopes: (0..32).map(|i| format!("{i:0>256}")).collect(),
            refresh_token: Some("r".repeat(16 * 1024)),
            token_source: Some(SIGNED_IN.to_owned()),
        };
        save(&path, &largest).unwrap();
        assert_eq!(load(&path).unwrap(), largest);
        // One that load would refuse is not written, and the file that was there is untouched.
        let too_large = Config {
            access_token: Some("a".repeat(70_000)),
            ..largest.clone()
        };
        let error = save(&path, &too_large).unwrap_err();
        assert!(error.contains("nothing was written"), "{error}");
        assert_eq!(load(&path).unwrap(), largest);
        // The boundary itself: exactly the limit is read back, one more is not written.
        let mut at_limit = Config {
            access_token: Some("a".to_owned()),
            ..Config::default()
        };
        // One character of token is in the text already.
        let base = serde_yaml::to_string(&at_limit).unwrap().len() as u64 - 1;
        at_limit.access_token = Some("a".repeat((MAX_FILE_BYTES - base) as usize));
        assert_eq!(
            serde_yaml::to_string(&at_limit).unwrap().len() as u64,
            MAX_FILE_BYTES
        );
        save(&path, &at_limit).unwrap();
        assert_eq!(load(&path).unwrap(), at_limit);
        at_limit.access_token = Some("a".repeat((MAX_FILE_BYTES - base) as usize + 1));
        assert!(save(&path, &at_limit).is_err());
    }

    #[test]
    fn the_lock_is_exclusive_private_and_released_when_dropped() {
        let d = dir("lock");
        let path = d.join("config.yaml");
        let first = lock_for(&path, std::time::Duration::from_millis(100)).unwrap();
        let mode = std::fs::metadata(lock_path(&path))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let started = std::time::Instant::now();
        assert!(matches!(
            lock_for(&path, std::time::Duration::from_millis(150)),
            Err(LockError::Busy)
        ));
        assert!(started.elapsed() >= std::time::Duration::from_millis(100));
        drop(first);
        assert!(lock_for(&path, std::time::Duration::from_millis(100)).is_ok());
        // The lock file stays.
        assert!(lock_path(&path).exists());
        // A link in its place is not followed.
        let victim = d.join("victim");
        std::fs::write(&victim, "precious").unwrap();
        std::fs::remove_file(lock_path(&path)).unwrap();
        symlink(&victim, lock_path(&path)).unwrap();
        assert!(matches!(
            lock_for(&path, std::time::Duration::from_millis(50)),
            Err(LockError::Failed(_))
        ));
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
    }
}
