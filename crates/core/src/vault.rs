//! Encryption at rest for everything the app keeps on disk (job history,
//! data cache, audit log). A random 256-bit data key lives in the OS keychain;
//! data is sealed with XChaCha20-Poly1305 (authenticated, random 192-bit
//! nonce per message). If the keychain is unavailable the app keeps working,
//! but callers are told so they can avoid writing plaintext.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, Key, KeyInit},
};

use crate::KEYRING_SERVICE;

const KEYRING_DATA_KEY: &str = "data-key";
/// File header: format name + version, so the format can change later.
const MAGIC: &[u8; 4] = b"SRv1";
/// Header of a file sealed under its own key (a migration project), not the app's data key.
const MAGIC_OWN_KEY: &[u8; 4] = b"SRp1";
const NONCE_LEN: usize = 24;

pub struct Vault {
    /// None until first use; then the key, or the reason it's unavailable.
    key: Mutex<Option<Result<XChaCha20Poly1305, String>>>,
}

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl Vault {
    pub fn new() -> Self {
        Self { key: Mutex::new(None) }
    }

    /// A vault with a fixed key, so tests never touch the system keychain.
    #[doc(hidden)]
    pub fn for_tests() -> Self {
        let cipher = XChaCha20Poly1305::new_from_slice(&[7u8; 32]).expect("a 32-byte key");
        Self { key: Mutex::new(Some(Ok(cipher))) }
    }

    /// The cipher, loading (or creating) the data key on first use.
    fn cipher(&self) -> Result<XChaCha20Poly1305, String> {
        let mut slot = self.key.lock().unwrap();
        if slot.is_none() {
            *slot = Some(load_or_create_key());
        }
        slot.as_ref().unwrap().clone()
    }

    /// Ok if encryption is available, else why not (for display in Settings).
    pub fn status(&self) -> Result<(), String> {
        self.cipher().map(|_| ())
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        seal_with(&self.cipher()?, plaintext)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        open_with(&self.cipher()?, sealed)
    }

    /// A new random key for one thing that must be destroyable on its own (a migration project).
    pub fn new_own_key() -> Vec<u8> {
        Key::<XChaCha20Poly1305>::generate().to_vec()
    }

    /// Seals under a key of its own instead of the app's data key. Destroying that key destroys the data, wherever
    /// copies of the file are left (a backup, a snapshot, blocks the disk didn't overwrite).
    pub fn seal_own(key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| "The data key has the wrong length".to_string())?;
        seal_magic(&cipher, MAGIC_OWN_KEY, plaintext)
    }

    pub fn open_own(key: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| "The data key has the wrong length".to_string())?;
        open_magic(&cipher, MAGIC_OWN_KEY, sealed)
    }

    /// Whether a file was sealed under a key of its own.
    pub fn is_own_key(sealed: &[u8]) -> bool {
        sealed.starts_with(MAGIC_OWN_KEY)
    }

    /// One sealed record as a single text line (for append-only logs).
    pub fn seal_line(&self, plaintext: &str) -> Result<String, String> {
        Ok(B64.encode(self.seal(plaintext.as_bytes())?))
    }

    pub fn open_line(&self, line: &str) -> Result<String, String> {
        let bytes = B64.decode(line.trim()).map_err(|e| format!("Bad record encoding: {e}"))?;
        String::from_utf8(self.open(&bytes)?).map_err(|e| e.to_string())
    }
}

fn load_or_create_key() -> Result<XChaCha20Poly1305, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_DATA_KEY).map_err(|e| format!("System keychain unavailable ({e})"))?;
    let bytes = match entry.get_password() {
        Ok(stored) => B64.decode(stored.trim()).map_err(|_| "The stored data key is corrupt".to_string())?,
        Err(keyring::Error::NoEntry) => {
            let key = Key::<XChaCha20Poly1305>::generate();
            entry
                .set_password(&B64.encode(key.as_slice()))
                .map_err(|e| format!("Couldn't save a data key to the system keychain ({e})"))?;
            key.to_vec()
        }
        Err(e) => return Err(format!("Couldn't read the data key from the system keychain ({e})")),
    };
    XChaCha20Poly1305::new_from_slice(&bytes).map_err(|_| "The stored data key has the wrong length".to_string())
}

fn seal_with(cipher: &XChaCha20Poly1305, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    seal_magic(cipher, MAGIC, plaintext)
}

fn seal_magic(cipher: &XChaCha20Poly1305, magic: &[u8; 4], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let nonce = XNonce::generate();
    let ciphertext = cipher.encrypt(&nonce, plaintext).map_err(|_| "Encryption failed".to_string())?;
    let mut out = Vec::with_capacity(magic.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn open_with(cipher: &XChaCha20Poly1305, sealed: &[u8]) -> Result<Vec<u8>, String> {
    open_magic(cipher, MAGIC, sealed)
}

fn open_magic(cipher: &XChaCha20Poly1305, magic: &[u8; 4], sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < MAGIC.len() + NONCE_LEN || &sealed[..MAGIC.len()] != magic {
        return Err("Not an encrypted N-voke file".into());
    }
    let nonce = XNonce::try_from(&sealed[MAGIC.len()..MAGIC.len() + NONCE_LEN]).map_err(|_| "Bad nonce".to_string())?;
    cipher
        .decrypt(&nonce, &sealed[MAGIC.len() + NONCE_LEN..])
        // Wrong key or tampered/corrupt data: AEAD authentication fails.
        .map_err(|_| "Couldn't decrypt (the data key changed or the file is damaged)".to_string())
}

/// Store names become file names, so only allow a safe character set.
pub fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !name.starts_with('.');
    if ok { Ok(()) } else { Err(format!("Invalid store name: {name}")) }
}

/// Writes atomically (temp file + rename) with owner-only permissions.
///
/// Each write gets its own temp file: two writes to the same path can run at
/// once (separate blocking threads), and sharing one temp file would let them
/// interleave bytes and leave a file that no longer decrypts.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), NEXT_TMP.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut f = private_file(&tmp)?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn create_private_dir(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Opens (creating) a file only the current user can read or write.
pub fn private_file(path: &Path) -> Result<fs::File, String> {
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}

/// Opens a file for appending with owner-only permissions on creation.
pub fn private_append(path: &Path) -> Result<fs::File, String> {
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}

/// Overwrites a file with random bytes, flushes it to disk and deletes it. The data is encrypted anyway; this only
/// removes what a copy of the file could be read from. On a solid-state or copy-on-write disk the old blocks may
/// survive an overwrite, so the guarantee is destroying the key (see `seal_own`), not this.
pub fn shred_file(path: &Path) -> Result<(), String> {
    let len = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let overwrite = (|| {
        let mut f = fs::OpenOptions::new().write(true).open(path)?;
        let mut left = len;
        while left > 0 {
            let chunk = Key::<XChaCha20Poly1305>::generate();
            let bytes = chunk.as_slice();
            let n = (left as usize).min(bytes.len());
            f.write_all(&bytes[..n])?;
            left -= n as u64;
        }
        f.sync_all()
    })();
    // A failed overwrite must not leave the file behind: remove it regardless, and say if it couldn't be overwritten.
    let removed = fs::remove_file(path).map_err(|e| e.to_string());
    removed?;
    overwrite.map_err(|e| format!("{} was removed but could not be overwritten first: {e}", path.display()))
}

/// Moves an unreadable file aside (keeping it for inspection) so the app can start fresh.
pub fn set_aside(path: &Path) -> PathBuf {
    let aside = path.with_extension("unreadable");
    let _ = fs::rename(path, &aside);
    aside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&Key::<XChaCha20Poly1305>::generate())
    }

    #[test]
    fn a_file_sealed_under_its_own_key_opens_only_with_that_key() {
        let key = Vault::new_own_key();
        let sealed = Vault::seal_own(&key, b"project data").unwrap();
        assert!(Vault::is_own_key(&sealed));
        assert!(!sealed.windows(12).any(|w| w == b"project data"));
        assert_eq!(Vault::open_own(&key, &sealed).unwrap(), b"project data");
        assert!(Vault::open_own(&Vault::new_own_key(), &sealed).is_err(), "another key can't open it");
        assert!(Vault::open_own(&key, &seal_with(&cipher(), b"x").unwrap()).is_err(), "and it isn't mistaken for an app-key file");
    }

    #[test]
    fn a_shredded_file_is_overwritten_first_and_then_gone() {
        let dir = std::env::temp_dir().join(format!("nvoke-shred-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.enc");
        fs::write(&path, vec![0xAAu8; 200_000]).unwrap();
        // Hold the file open to see what the overwrite wrote before the name was removed.
        let held = fs::File::open(&path).unwrap();
        shred_file(&path).unwrap();
        assert!(!path.exists());
        let mut seen = Vec::new();
        std::io::Read::read_to_end(&mut &held, &mut seen).unwrap();
        assert_eq!(seen.len(), 200_000, "the same length was overwritten");
        assert!(seen.iter().any(|b| *b != 0xAA) && seen.iter().filter(|b| **b == 0xAA).count() < 10_000, "with something else");
        assert!(shred_file(&path).is_ok(), "a file that isn't there is fine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_and_uses_a_fresh_nonce_each_time() {
        let c = cipher();
        let a = seal_with(&c, b"job history").unwrap();
        let b = seal_with(&c, b"job history").unwrap();
        assert_ne!(a, b, "same plaintext must not produce the same ciphertext");
        assert_eq!(open_with(&c, &a).unwrap(), b"job history");
        assert!(!a.windows(11).any(|w| w == b"job history"), "plaintext must not appear in the output");
    }

    #[test]
    fn detects_tampering_and_wrong_keys() {
        let c = cipher();
        let mut sealed = seal_with(&c, b"secret output").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(open_with(&c, &sealed).is_err());
        let sealed = seal_with(&c, b"secret output").unwrap();
        assert!(open_with(&cipher(), &sealed).is_err());
        assert!(open_with(&c, b"{\"plain\": true}").is_err());
    }

    #[test]
    fn concurrent_writes_to_one_path_never_mix() {
        let dir = std::env::temp_dir().join(format!("vault-test-{}", std::process::id()));
        let path = dir.join("jobs.enc");
        let writers: Vec<_> = (0..8u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        write_private(&path, &vec![i; 64 * 1024 + i as usize]).unwrap();
                    }
                })
            })
            .collect();
        writers.into_iter().for_each(|w| w.join().unwrap());
        let bytes = fs::read(&path).unwrap();
        let first = bytes[0];
        assert_eq!(bytes.len(), 64 * 1024 + first as usize);
        assert!(bytes.iter().all(|&b| b == first), "file holds bytes from more than one write");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "temp files left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_allows_safe_store_names() {
        assert!(validate_name("jobs").is_ok());
        assert!(validate_name("query-cache.v1").is_ok());
        assert!(validate_name("../etc/passwd").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("").is_err());
    }
}
