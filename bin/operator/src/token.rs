//! A token read from the reference the configuration names, **re-read when it changes** (ANR-1192): a deployment that
//! rotates a Secret no longer needs to restart the operator, the API or the meter.
//!
//! A token is never printed: this module holds the text, and every type that carries one out of here has no `Display`.

use std::sync::Mutex;
use std::time::SystemTime;

use aap_trace_otlp::SecretRef;

/// The reference the configuration names, and the last text it gave.
///
/// [`TokenFile::get`] re-reads a file when its modification time or size changed since the last read; a read that fails
/// **keeps the last text it read**, because a Secret volume is briefly inconsistent while a rotation is applied and a
/// meter or a registry must not stop for that. A variable is read each time (it is already in memory).
pub struct TokenFile {
    reference: SecretRef,
    cached: Mutex<Option<Cached>>,
}

struct Cached {
    /// `None` for a text seeded from the start-time read: the next `get` re-reads the file, so a rotation that landed
    /// between that read and the seed is never cached under a stamp it does not belong to. `Some` for a text `get` read
    /// itself, with the stamp taken **before** the read.
    stamp: Option<Stamp>,
    text: String,
}

/// What says a file changed: its modification time and its size (a Secret's payload is replaced, so both move).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp(SystemTime, u64);

impl std::fmt::Debug for TokenFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenFile")
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}

impl TokenFile {
    /// The reference, with nothing read yet.
    #[must_use]
    pub fn new(reference: SecretRef) -> Self {
        Self {
            reference,
            cached: Mutex::new(None),
        }
    }

    /// Seed the cache with a text that was just read and validated (the start-time read): the source answers it even if the
    /// file vanishes before the first request. The seeded text carries **no stamp**, so the next `get` re-reads the file
    /// if it is readable (a rotation between the caller's read and this call is therefore picked up) and serves the seeded
    /// text only if the file cannot be read.
    pub fn seed(&self, text: &str) {
        if let Ok(mut cached) = self.cached.lock() {
            *cached = Some(Cached {
                stamp: None,
                text: text.to_owned(),
            });
        }
    }

    /// The token as the reference gives it now: the file's text when it changed, the last text it gave when the file
    /// cannot be read, and `None` when it was never readable or the variable is not set.
    ///
    /// A text is returned as read: the caller validates it, as it does at start.
    #[must_use]
    pub fn get(&self) -> Option<String> {
        match &self.reference {
            SecretRef::Env(name) => std::env::var(name).ok(),
            SecretRef::File(path) => {
                let now = stamp(path);
                if let Some(cached) = self.cached()
                    && cached.stamp.is_some()
                    && cached.stamp == now
                {
                    return Some(cached.text);
                }
                match std::fs::read_to_string(path) {
                    Ok(text) => {
                        let stamp = now.unwrap_or(Stamp(SystemTime::UNIX_EPOCH, 0));
                        if let Ok(mut cached) = self.cached.lock() {
                            *cached = Some(Cached {
                                stamp: Some(stamp),
                                text: text.clone(),
                            });
                        }
                        Some(text)
                    }
                    // The rotation's window: the last token it read, not none.
                    Err(_) => self.cached().map(|cached| cached.text),
                }
            }
        }
    }

    fn cached(&self) -> Option<Cached> {
        self.cached.lock().ok().and_then(|cached| {
            cached.as_ref().map(|cached| Cached {
                stamp: cached.stamp,
                text: cached.text.clone(),
            })
        })
    }
}

/// The file's stamp, or `None` when it cannot be read (a missing file, or a platform without one).
fn stamp(path: &std::path::Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp(meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Write as _;

    fn write(path: &std::path::Path, text: &str) {
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn a_file_is_read_and_a_change_is_picked_up() {
        let dir = std::env::temp_dir().join(format!("aap-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        write(&path, "first");
        let token = TokenFile::new(SecretRef::File(path.clone()));
        assert_eq!(token.get().as_deref(), Some("first"));
        // The same text is served from the cache (the stamp did not move).
        assert_eq!(token.get().as_deref(), Some("first"));
        write(&path, "second-token");
        assert_eq!(token.get().as_deref(), Some("second-token"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_seeded_text_never_wins_over_a_readable_file() {
        // The seed carries no stamp: a rotation between the caller's read and the seed is picked up by the next `get`.
        let dir = std::env::temp_dir().join(format!("aap-token-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        write(&path, "rotated");
        let token = TokenFile::new(SecretRef::File(path.clone()));
        token.seed("start-time");
        assert_eq!(token.get().as_deref(), Some("rotated"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_seeded_text_is_kept_when_the_file_is_gone_before_the_first_get() {
        // The start-time read validated a text; the file vanishes before the first request, and the source still answers it.
        let token = TokenFile::new(SecretRef::File(std::path::PathBuf::from(
            "/nonexistent/aap/seeded",
        )));
        token.seed("start-time");
        assert_eq!(token.get().as_deref(), Some("start-time"));
    }

    #[test]
    fn a_file_that_disappears_keeps_the_last_token_it_read() {
        let dir = std::env::temp_dir().join(format!("aap-token-gone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("token");
        write(&path, "kept");
        let token = TokenFile::new(SecretRef::File(path.clone()));
        assert_eq!(token.get().as_deref(), Some("kept"));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(token.get().as_deref(), Some("kept"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_was_never_readable_gives_none() {
        let token = TokenFile::new(SecretRef::File(std::path::PathBuf::from(
            "/nonexistent/aap/token",
        )));
        assert_eq!(token.get(), None);
    }

    #[test]
    fn the_debug_prints_the_reference_and_never_a_value() {
        let token = TokenFile::new(SecretRef::Env("AAP_TOKEN_THAT_IS_SET".to_owned()));
        // SAFETY: the test sets and removes its own variable, and nothing else reads it.
        unsafe { std::env::set_var("AAP_TOKEN_THAT_IS_SET", "s3cret-value") };
        assert_eq!(token.get().as_deref(), Some("s3cret-value"));
        let printed = format!("{token:?}");
        assert!(!printed.contains("s3cret-value"), "{printed}");
        assert!(printed.contains("AAP_TOKEN_THAT_IS_SET"), "{printed}");
        unsafe { std::env::remove_var("AAP_TOKEN_THAT_IS_SET") };
    }
}
