//! The key file: the owner's API keys, encrypted with a key derived from
//! their passphrase, or not stored at all if they choose.
//!
//! Argon2id derives a 256-bit key from the passphrase and a random 16-byte
//! salt, and XChaCha20-Poly1305 with a random 24-byte nonce encrypts the
//! API keys. The header before the nonce (format version, Argon2 settings,
//! and salt) is the associated data, so changing any of it, to weaken the
//! settings say, makes the file fail to open. A wrong passphrase and a
//! damaged file give the same error.
//!
//! Decrypted keys are held in types that wipe their memory when dropped
//! and never print their contents.

#![forbid(unsafe_code)]

use std::fmt;
use std::io::{Read, Write};
use std::path::Path;

use argon2::{Algorithm, Argon2, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, Zeroizing};

/// What every key file starts with.
const MAGIC: &[u8; 8] = b"MLBXAKEY";
/// The format written; a file of a later one was made by a newer app.
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
/// Magic, version, the three Argon2 settings, and the salt.
const HEADER_LEN: usize = MAGIC.len() + 1 + 3 * 4 + SALT_LEN;
/// Far more than two API keys need; a larger file is not read.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// The longest API key accepted. Providers' keys are well under this.
const MAX_KEY_BYTES: usize = 1024;

/// Why the key file could not be made, read, or opened.
#[derive(Debug)]
pub enum KeyFileError {
    /// The passphrase is wrong, or the file is damaged; the two cannot be
    /// told apart, by design.
    CannotOpen,
    /// The file was written by a newer version of the app.
    NewerVersion,
    /// The system's random number source failed.
    NoRandomness,
    Io(std::io::Error),
}

impl fmt::Display for KeyFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CannotOpen => f.write_str("the passphrase is wrong or the key file is damaged"),
            Self::NewerVersion => {
                f.write_str("the key file was saved by a newer version of the app")
            }
            Self::NoRandomness => f.write_str("the system could not supply random numbers"),
            Self::Io(e) => write!(f, "the key file could not be read or saved: {e}"),
        }
    }
}

impl std::error::Error for KeyFileError {}

impl From<std::io::Error> for KeyFileError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// One provider's API key. Its text is wiped from memory when it is
/// dropped and never shown by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(Zeroizing<String>);

impl ApiKey {
    /// A key as pasted, without the spaces around it; `None` if it is
    /// empty, too long, or holds anything but printable ASCII, which no
    /// provider's keys do.
    pub fn new(text: String) -> Option<Self> {
        let text = Zeroizing::new(text);
        let key = text.trim();
        let valid = !key.is_empty()
            && key.len() <= MAX_KEY_BYTES
            && key.bytes().all(|b| b.is_ascii_graphic());
        valid.then(|| Self(Zeroizing::new(key.to_owned())))
    }

    /// The key itself, for the request that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(hidden)")
    }
}

/// The passphrase the key file is locked with, wiped from memory when
/// dropped and never shown by `Debug`.
pub struct Passphrase(Zeroizing<String>);

impl Passphrase {
    /// `None` for an empty passphrase. Spaces are kept: they may be part
    /// of it.
    pub fn new(text: String) -> Option<Self> {
        let text = Zeroizing::new(text);
        (!text.is_empty()).then_some(Self(text))
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase(hidden)")
    }
}

/// The API keys the file holds, one per provider.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiKeys {
    pub anthropic: Option<ApiKey>,
    pub openrouter: Option<ApiKey>,
}

/// The Argon2id settings: memory in KiB, passes over it, and lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    memory_kib: u32,
    passes: u32,
    lanes: u32,
}

impl KdfParams {
    /// What new key files use: 512 MiB, 3 passes, 1 lane, which takes
    /// about a second to open on a 2026 desktop.
    pub const DEFAULT: Self = Self {
        memory_kib: 512 * 1024,
        passes: 3,
        lanes: 1,
    };

    /// The weakest settings a key file may have: 64 MiB, 3 passes, 1 lane.
    pub const MINIMUM: Self = Self {
        memory_kib: 64 * 1024,
        passes: 3,
        lanes: 1,
    };

    /// Settings no weaker than [`Self::MINIMUM`], and not so heavy that a
    /// file could make opening it exhaust the machine: up to 1 GiB, 64
    /// passes, and 8 lanes. `None` outside that.
    pub fn new(memory_kib: u32, passes: u32, lanes: u32) -> Option<Self> {
        let params = Self {
            memory_kib,
            passes,
            lanes,
        };
        params.acceptable().then_some(params)
    }

    pub fn memory_kib(self) -> u32 {
        self.memory_kib
    }

    pub fn passes(self) -> u32 {
        self.passes
    }

    pub fn lanes(self) -> u32 {
        self.lanes
    }

    fn acceptable(self) -> bool {
        (Self::MINIMUM.memory_kib..=1024 * 1024).contains(&self.memory_kib)
            && (Self::MINIMUM.passes..=64).contains(&self.passes)
            && (1..=8).contains(&self.lanes)
    }

    /// The 256-bit key derived from `passphrase` and `salt`.
    fn derive(
        self,
        passphrase: &Passphrase,
        salt: &[u8],
    ) -> Result<Zeroizing<[u8; 32]>, KeyFileError> {
        let params = argon2::Params::new(self.memory_kib, self.passes, self.lanes, Some(32))
            .map_err(|_| KeyFileError::CannotOpen)?;
        let mut key = Zeroizing::new([0; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.0.as_bytes(), salt, key.as_mut())
            .map_err(|_| KeyFileError::CannotOpen)?;
        Ok(key)
    }
}

fn random<const N: usize>() -> Result<[u8; N], KeyFileError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| KeyFileError::NoRandomness)?;
    Ok(bytes)
}

/// The keys laid out for encrypting: for each key held, its provider's
/// tag, its length as two bytes, and its text.
fn plaintext(keys: &ApiKeys) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::new());
    for (tag, key) in [(1u8, &keys.anthropic), (2, &keys.openrouter)] {
        if let Some(key) = key {
            let text = key.expose().as_bytes();
            out.push(tag);
            // Keys are at most MAX_KEY_BYTES long, which fits.
            out.extend_from_slice(&u16::try_from(text.len()).unwrap_or(u16::MAX).to_le_bytes());
            out.extend_from_slice(text);
        }
    }
    out
}

/// Undoes [`plaintext`]. The text was authenticated, so a failure here
/// means the file was written wrongly, not tampered with.
fn keys_from(plain: &[u8]) -> Result<ApiKeys, KeyFileError> {
    let mut keys = ApiKeys::default();
    let mut rest = plain;
    while let Some((&tag, after)) = rest.split_first() {
        let (len, after) = after
            .split_first_chunk::<2>()
            .ok_or(KeyFileError::CannotOpen)?;
        let len = usize::from(u16::from_le_bytes(*len));
        let (text, after) = after
            .split_at_checked(len)
            .ok_or(KeyFileError::CannotOpen)?;
        let text = std::str::from_utf8(text).map_err(|_| KeyFileError::CannotOpen)?;
        let key = ApiKey::new(text.to_owned()).ok_or(KeyFileError::CannotOpen)?;
        let slot = match tag {
            1 => &mut keys.anthropic,
            2 => &mut keys.openrouter,
            _ => return Err(KeyFileError::CannotOpen),
        };
        if slot.replace(key).is_some() {
            return Err(KeyFileError::CannotOpen);
        }
        rest = after;
    }
    Ok(keys)
}

/// The keys encrypted under `passphrase`, as the key file's bytes.
pub fn seal(
    keys: &ApiKeys,
    passphrase: &Passphrase,
    params: KdfParams,
) -> Result<Vec<u8>, KeyFileError> {
    let salt: [u8; SALT_LEN] = random()?;
    let nonce: [u8; NONCE_LEN] = random()?;
    let mut file = Vec::with_capacity(HEADER_LEN + NONCE_LEN + 128);
    file.extend_from_slice(MAGIC);
    file.push(VERSION);
    for n in [params.memory_kib, params.passes, params.lanes] {
        file.extend_from_slice(&n.to_le_bytes());
    }
    file.extend_from_slice(&salt);
    let key = params.derive(passphrase, &salt)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| KeyFileError::CannotOpen)?;
    let plain = plaintext(keys);
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &plain,
                aad: &file[..HEADER_LEN],
            },
        )
        .map_err(|_| KeyFileError::CannotOpen)?;
    file.extend_from_slice(&nonce);
    file.extend_from_slice(&sealed);
    Ok(file)
}

/// The keys in a key file's bytes, if `passphrase` opens it.
pub fn open(file: &[u8], passphrase: &Passphrase) -> Result<ApiKeys, KeyFileError> {
    let (header, rest) = file
        .split_at_checked(HEADER_LEN)
        .ok_or(KeyFileError::CannotOpen)?;
    let (magic, header_rest) = header.split_at(MAGIC.len());
    if magic != MAGIC {
        return Err(KeyFileError::CannotOpen);
    }
    let (&version, settings) = header_rest.split_first().ok_or(KeyFileError::CannotOpen)?;
    if version > VERSION {
        return Err(KeyFileError::NewerVersion);
    }
    if version != VERSION {
        return Err(KeyFileError::CannotOpen);
    }
    let number = |n: usize| -> u32 {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(&settings[4 * n..4 * n + 4]);
        u32::from_le_bytes(bytes)
    };
    // Checked before any work, so a file cannot make opening it slow or
    // hungry, nor be opened with weakened settings.
    let params = KdfParams::new(number(0), number(1), number(2)).ok_or(KeyFileError::CannotOpen)?;
    let salt = &settings[12..];
    let (nonce, sealed) = rest
        .split_first_chunk::<NONCE_LEN>()
        .ok_or(KeyFileError::CannotOpen)?;
    let key = params.derive(passphrase, salt)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| KeyFileError::CannotOpen)?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(*nonce),
                Payload {
                    msg: sealed,
                    aad: header,
                },
            )
            .map_err(|_| KeyFileError::CannotOpen)?,
    );
    keys_from(&plain)
}

/// Writes the key file at `path`, readable and writable by the owner
/// only. It is written whole to a new file beside it, then renamed over
/// it, so a crash leaves either the old file or the new one.
pub fn save(path: &Path, file: &[u8]) -> Result<(), KeyFileError> {
    use std::os::unix::fs::OpenOptionsExt;

    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("the key file needs a file name"))?;
    let suffix: [u8; 8] = random()?;
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let mut temp_name = name.to_os_string();
    temp_name.push(format!(".{suffix}.tmp"));
    let temp = dir.join(temp_name);
    let written = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        out.write_all(file)?;
        out.sync_all()?;
        std::fs::rename(&temp, path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(written?)
}

/// The key file's bytes, or `None` if there is no key file.
pub fn load(path: &Path) -> Result<Option<Vec<u8>>, KeyFileError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
        bytes.zeroize();
        return Err(KeyFileError::CannotOpen);
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests;
