//! Encrypted vault for Hyper Clips recordings.
//!
//! A vault is a directory that stores clip files encrypted at rest under a
//! single password. Files are encrypted and decrypted in a streaming fashion,
//! so arbitrarily large videos never need to fit in memory.
//!
//! # On-disk format (version 1)
//!
//! ```text
//! <vault dir>/
//!   vault.json        header (plaintext JSON)
//!   index.hcv         encrypted index of entries
//!   files/<id>.hcv    one encrypted file per entry; <id> = 32 random hex chars
//! ```
//!
//! * **`vault.json`** holds the format name and version, the Argon2id (v1.3)
//!   parameters (`m_cost_kib`, `t_cost`, `p_cost`; default 64 MiB / 3 / 1),
//!   a random 16-byte salt and the *wrapped master key*: a random 256-bit
//!   master key sealed with XChaCha20-Poly1305 under the key derived from the
//!   password (random 24-byte nonce). The KDF parameters and salt are bound
//!   as associated data. Changing the password only re-wraps the master key;
//!   no file is re-encrypted.
//! * **`index.hcv`** is `"HCVI" || version(1) || nonce(24) || ciphertext`,
//!   an XChaCha20-Poly1305 seal (master key) of a JSON document listing every
//!   entry: id, original file name, size, added timestamp (unix seconds) and
//!   optional JSON metadata. File names therefore never appear in plaintext.
//! * **`files/<id>.hcv`** starts with a 28-byte header
//!   `"HCVF" || version(1) || chunk_size(u32 BE) || nonce_prefix(19)` followed
//!   by chunks of `chunk_size` plaintext bytes (1 MiB by default), each
//!   encrypted with XChaCha20-Poly1305 under the master key with nonce
//!   `nonce_prefix || counter(u32 BE) || last_flag(1 byte)` (the STREAM
//!   construction). The file header and the entry id are associated data of
//!   every chunk. All chunks but the last are full; the last is shorter
//!   (possibly empty), so truncation, reordering, chunk removal, header edits
//!   and swapping files between entries are all detected.
//!
//! New files are written to a temporary name, fsynced and renamed into place.
//!
//! # Threat model
//!
//! The vault protects the *confidentiality and integrity of clips at rest*:
//! someone who obtains the vault directory (stolen disk, copied folder,
//! cloud backup) without the password cannot read clip contents or names, and
//! cannot modify a clip without detection. The password is protected against
//! offline guessing only by Argon2id's cost; weak passwords remain weak.
//!
//! It does **not** protect against:
//!
//! * malware or another user with access to the machine while the vault is
//!   unlocked (keys are in process memory; extracted/exported files are
//!   plaintext on disk);
//! * metadata leakage: the number of entries, the approximate size of each
//!   clip (ciphertext size ≈ plaintext size), and file modification times are
//!   visible;
//! * rollback: an attacker can replace the whole index or a file with an
//!   older, validly-encrypted version from the same vault;
//! * remnants of originals: `delete_original` performs a normal file delete,
//!   which does not securely erase data on SSDs or journaling filesystems.
//!
//! Only key material is zeroized in memory (via [`zeroize::Zeroizing`]);
//! decrypted clip data passes through ordinary buffers.
//!
//! The vault is not designed for concurrent use by multiple processes; within
//! one process a [`Vault`] may be shared between threads.

mod crypto;
mod error;

use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::Zeroizing;

pub use crypto::KdfParams;
pub use error::{Error, Result};

use crypto::{KEY_LEN, SALT_LEN, SecretKey};

/// Name of the plaintext header file inside the vault directory.
pub const HEADER_FILE: &str = "vault.json";
/// Name of the encrypted index file inside the vault directory.
pub const INDEX_FILE: &str = "index.hcv";
/// Subdirectory holding encrypted entry files.
pub const FILES_DIR: &str = "files";
/// Extension of encrypted entry files.
pub const FILE_EXT: &str = "hcv";

const FORMAT_NAME: &str = "hyper-clips-vault";
const FORMAT_VERSION: u32 = 1;
const KDF_ALGORITHM: &str = "argon2id";
const INDEX_MAGIC: &[u8; 4] = b"HCVI";
const INDEX_VERSION: u8 = 1;
const INDEX_AAD: &[u8] = b"hc-vault/v1/index";

// ---------------------------------------------------------------------------
// Entry ids and entries
// ---------------------------------------------------------------------------

/// Random 128-bit identifier of a vault entry. Displayed as 32 lower-case hex
/// characters, which is also the stem of its encrypted file name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId([u8; 16]);

impl EntryId {
    fn random() -> Result<Self> {
        Ok(Self(crypto::random_array()?))
    }

    /// Raw id bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crypto::to_hex(&self.0))
    }
}

impl fmt::Debug for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntryId({self})")
    }
}

impl FromStr for EntryId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = crypto::from_hex(s).ok_or(Error::NotFound)?;
        let arr: [u8; 16] = bytes.try_into().map_err(|_| Error::NotFound)?;
        Ok(Self(arr))
    }
}

impl Serialize for EntryId {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EntryId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse()
            .map_err(|_| serde::de::Error::custom("invalid entry id"))
    }
}

/// A file stored in the vault, as recorded in the encrypted index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Unique id.
    pub id: EntryId,
    /// Original file name (no directory components).
    pub name: String,
    /// Plaintext size in bytes.
    pub size: u64,
    /// When the entry was added, in seconds since the Unix epoch.
    pub added_unix: u64,
    /// Free-form application metadata (game, tags, ...). `Null` if unset.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    entries: Vec<Entry>,
}

impl Index {
    fn find(&self, id: EntryId) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn find_mut(&mut self, id: EntryId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Header {
    format: String,
    version: u32,
    kdf: KdfHeader,
    wrapped_key: String,
}

#[derive(Serialize, Deserialize)]
struct KdfHeader {
    algorithm: String,
    #[serde(flatten)]
    params: KdfParams,
    salt: String,
}

/// Associated data binding the wrapped key to its KDF parameters and salt.
fn wrap_aad(params: KdfParams, salt: &[u8; SALT_LEN]) -> Vec<u8> {
    format!(
        "hc-vault/v1/master-key|{KDF_ALGORITHM}|v19|m={}|t={}|p={}|salt={}",
        params.m_cost_kib,
        params.t_cost,
        params.p_cost,
        crypto::to_hex(salt)
    )
    .into_bytes()
}

fn build_header(master: &SecretKey, password: &str, params: KdfParams) -> Result<Header> {
    let salt: [u8; SALT_LEN] = crypto::random_array()?;
    let kek = crypto::derive_kek(password.as_bytes(), &salt, params)?;
    let wrapped = crypto::seal(&kek, &wrap_aad(params, &salt), master.as_slice())?;
    Ok(Header {
        format: FORMAT_NAME.into(),
        version: FORMAT_VERSION,
        kdf: KdfHeader {
            algorithm: KDF_ALGORITHM.into(),
            params,
            salt: crypto::to_hex(&salt),
        },
        wrapped_key: crypto::to_hex(&wrapped),
    })
}

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

fn random_suffix() -> Result<String> {
    Ok(crypto::to_hex(&crypto::random_array::<8>()?))
}

/// Writes `bytes` to `path` via a fsynced temporary file and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", random_suffix()?));
    let res = (|| -> io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(res?)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Turns a stored name into a safe single path component.
fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "clip".into()
    } else {
        trimmed.into()
    }
}

fn map_not_found(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::NotFound {
        Error::NotFound
    } else {
        Error::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Vault
// ---------------------------------------------------------------------------

/// An unlocked vault. Holds the master key in zeroizing memory; dropping the
/// value (or calling [`Vault::lock`]) wipes the key and removes temporary
/// exports created by [`Vault::export_temp`].
pub struct Vault {
    dir: PathBuf,
    master: SecretKey,
    params: KdfParams,
    index: Mutex<Index>,
    temp_exports: Mutex<Vec<PathBuf>>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("dir", &self.dir)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Vault {
    /// Returns `true` if `dir` contains a vault header.
    pub fn exists(dir: impl AsRef<Path>) -> bool {
        dir.as_ref().join(HEADER_FILE).is_file()
    }

    /// Creates a new vault in `dir` (created if missing) with the default
    /// Argon2id parameters. Fails with [`Error::AlreadyExists`] if a vault is
    /// already there.
    pub fn create(dir: impl AsRef<Path>, password: &str) -> Result<Self> {
        Self::create_with_params(dir, password, KdfParams::default())
    }

    /// Like [`Vault::create`] with explicit Argon2id parameters (lower them
    /// only for tests).
    pub fn create_with_params(
        dir: impl AsRef<Path>,
        password: &str,
        params: KdfParams,
    ) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        if Self::exists(&dir) {
            return Err(Error::AlreadyExists);
        }
        params.validate()?;
        fs::create_dir_all(dir.join(FILES_DIR))?;
        let vault = Self {
            master: crypto::random_key()?,
            dir,
            params,
            index: Mutex::new(Index::default()),
            temp_exports: Mutex::new(Vec::new()),
        };
        vault.save_index(&guard(&vault.index))?;
        // The header is written last so `exists` only sees complete vaults.
        vault.write_header(password, params)?;
        Ok(vault)
    }

    /// Unlocks the vault in `dir`. Returns [`Error::WrongPassword`] if the
    /// password does not decrypt the master key.
    pub fn unlock(dir: impl AsRef<Path>, password: &str) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        let raw = fs::read(dir.join(HEADER_FILE)).map_err(map_not_found)?;
        let header: Header = serde_json::from_slice(&raw)
            .map_err(|e| Error::Format(format!("unreadable header: {e}")))?;
        if header.format != FORMAT_NAME {
            return Err(Error::Format("not a Hyper Clips vault".into()));
        }
        if header.version != FORMAT_VERSION {
            return Err(Error::Format(format!(
                "unsupported vault version {}",
                header.version
            )));
        }
        if header.kdf.algorithm != KDF_ALGORITHM {
            return Err(Error::Format("unsupported key derivation algorithm".into()));
        }
        let params = header.kdf.params;
        let salt: [u8; SALT_LEN] = crypto::from_hex(&header.kdf.salt)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| Error::Format("invalid salt".into()))?;
        let wrapped = crypto::from_hex(&header.wrapped_key)
            .ok_or_else(|| Error::Format("invalid wrapped key".into()))?;

        let kek = crypto::derive_kek(password.as_bytes(), &salt, params)?;
        let plain =
            crypto::open(&kek, &wrap_aad(params, &salt), &wrapped).ok_or(Error::WrongPassword)?;
        let mut master = Zeroizing::new([0u8; KEY_LEN]);
        if plain.len() != KEY_LEN {
            return Err(Error::Corrupt);
        }
        master.copy_from_slice(&plain);

        let vault = Self {
            dir,
            master,
            params,
            index: Mutex::new(Index::default()),
            temp_exports: Mutex::new(Vec::new()),
        };
        *guard(&vault.index) = vault.load_index()?;
        Ok(vault)
    }

    /// Re-wraps the master key under `new_password` with a fresh salt. Stored
    /// files are untouched. The header is replaced atomically.
    pub fn change_password(&mut self, new_password: &str) -> Result<()> {
        self.write_header(new_password, self.params)
    }

    /// Locks the vault: removes temporary exports and wipes the key.
    pub fn lock(self) {
        drop(self);
    }

    /// The vault directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The Argon2id parameters stored in the header.
    pub fn kdf_params(&self) -> KdfParams {
        self.params
    }

    /// Encrypts `src` into the vault and returns the new entry id. See
    /// [`Vault::add_file_with_metadata`].
    pub fn add_file(&self, src: &Path, delete_original: bool) -> Result<EntryId> {
        self.add_file_with_metadata(src, delete_original, serde_json::Value::Null)
    }

    /// Encrypts `src` into the vault (streaming, 1 MiB chunks) and records it
    /// in the index with `metadata`.
    ///
    /// The ciphertext is written to a temporary file, fsynced and renamed
    /// into place before the index is updated. If `delete_original` is set,
    /// the source is removed only after all of that succeeded; if removing it
    /// fails, [`Error::OriginalNotDeleted`] is returned and the entry is kept.
    pub fn add_file_with_metadata(
        &self,
        src: &Path,
        delete_original: bool,
        metadata: serde_json::Value,
    ) -> Result<EntryId> {
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::Io(io::Error::other("source path has no file name")))?;
        let mut input = File::open(src).map_err(map_not_found)?;
        if !input.metadata()?.is_file() {
            return Err(Error::Io(io::Error::other("source is not a regular file")));
        }

        let id = EntryId::random()?;
        let final_path = self.entry_path(id);
        let tmp_path = final_path.with_extension(format!("{FILE_EXT}.part"));
        let written = (|| -> Result<u64> {
            let mut out = BufWriter::with_capacity(256 * 1024, File::create_new(&tmp_path)?);
            let n = crypto::encrypt_stream(
                &self.master,
                id.as_bytes(),
                &mut input,
                &mut out,
                crypto::DEFAULT_CHUNK_SIZE,
            )?;
            let file = out.into_inner().map_err(io::IntoInnerError::into_error)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp_path, &final_path)?;
            Ok(n)
        })();
        let size = match written {
            Ok(n) => n,
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(e);
            }
        };
        drop(input);

        {
            let mut index = guard(&self.index);
            index.entries.push(Entry {
                id,
                name,
                size,
                added_unix: now_unix(),
                metadata,
            });
            if let Err(e) = self.save_index(&index) {
                index.entries.retain(|x| x.id != id);
                drop(index);
                let _ = fs::remove_file(&final_path);
                return Err(e);
            }
        }

        if delete_original {
            fs::remove_file(src).map_err(|source| Error::OriginalNotDeleted { id, source })?;
        }
        Ok(id)
    }

    /// All entries, oldest first.
    pub fn list(&self) -> Vec<Entry> {
        let mut v = guard(&self.index).entries.clone();
        v.sort_by(|a, b| {
            a.added_unix
                .cmp(&b.added_unix)
                .then_with(|| a.name.cmp(&b.name))
        });
        v
    }

    /// Looks up a single entry.
    pub fn get(&self, id: EntryId) -> Option<Entry> {
        guard(&self.index).find(id).cloned()
    }

    /// Decrypts entry `id` to `dest` (overwriting it). Data is streamed into a
    /// temporary file next to `dest` that is renamed into place only after
    /// every chunk authenticated; on failure the partial output is deleted
    /// and [`Error::Corrupt`] is returned for tampered/truncated data.
    pub fn extract(&self, id: EntryId, dest: &Path) -> Result<()> {
        let entry = self.get(id).ok_or(Error::NotFound)?;
        let mut input = BufReader::with_capacity(
            256 * 1024,
            File::open(self.entry_path(id)).map_err(map_not_found)?,
        );

        let dest_name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| Error::Io(io::Error::other("destination has no file name")))?;
        let tmp = dest.with_file_name(format!(".{dest_name}.hcv-part-{}", random_suffix()?));
        let res = (|| -> Result<()> {
            let mut out = BufWriter::with_capacity(256 * 1024, File::create_new(&tmp)?);
            let n = crypto::decrypt_stream(&self.master, id.as_bytes(), &mut input, &mut out)?;
            if n != entry.size {
                return Err(Error::Corrupt);
            }
            let file = out.into_inner().map_err(io::IntoInnerError::into_error)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, dest)?;
            Ok(())
        })();
        if res.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        res
    }

    /// Decrypts and authenticates entry `id` without writing plaintext
    /// anywhere.
    pub fn verify(&self, id: EntryId) -> Result<()> {
        let entry = self.get(id).ok_or(Error::NotFound)?;
        let mut input = BufReader::with_capacity(
            256 * 1024,
            File::open(self.entry_path(id)).map_err(map_not_found)?,
        );
        let n = crypto::decrypt_stream(&self.master, id.as_bytes(), &mut input, &mut io::sink())?;
        if n != entry.size {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    /// Decrypts entry `id` into a fresh private directory under the system
    /// temp dir (keeping its original file name) and returns the path, e.g.
    /// for playback. The directory is removed by [`Vault::clear_temp_exports`],
    /// [`Vault::lock`] or drop (best effort: files still open elsewhere may
    /// survive). The exported file is plaintext on disk while it exists.
    pub fn export_temp(&self, id: EntryId) -> Result<PathBuf> {
        let entry = self.get(id).ok_or(Error::NotFound)?;
        let dir = std::env::temp_dir().join(format!("hc-vault-export-{}", random_suffix()?));
        fs::create_dir(&dir)?;
        let path = dir.join(sanitize_file_name(&entry.name));
        match self.extract(id, &path) {
            Ok(()) => {
                guard(&self.temp_exports).push(dir);
                Ok(path)
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&dir);
                Err(e)
            }
        }
    }

    /// Deletes every directory created by [`Vault::export_temp`] (best
    /// effort; directories that cannot be removed yet are kept for a later
    /// attempt).
    pub fn clear_temp_exports(&self) {
        guard(&self.temp_exports).retain(|d| fs::remove_dir_all(d).is_err() && d.exists());
    }

    /// Removes entry `id` from the index and deletes its encrypted file.
    pub fn remove(&self, id: EntryId) -> Result<()> {
        {
            let mut index = guard(&self.index);
            let pos = index
                .entries
                .iter()
                .position(|e| e.id == id)
                .ok_or(Error::NotFound)?;
            let removed = index.entries.remove(pos);
            if let Err(e) = self.save_index(&index) {
                index.entries.insert(pos, removed);
                return Err(e);
            }
        }
        match fs::remove_file(self.entry_path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Changes the stored original file name of entry `id`.
    pub fn rename(&self, id: EntryId, new_name: &str) -> Result<()> {
        self.update_entry(id, |e| e.name = new_name.to_owned())
    }

    /// Replaces the metadata of entry `id`.
    pub fn set_metadata(&self, id: EntryId, metadata: serde_json::Value) -> Result<()> {
        self.update_entry(id, |e| e.metadata = metadata)
    }

    fn update_entry(&self, id: EntryId, f: impl FnOnce(&mut Entry)) -> Result<()> {
        let mut index = guard(&self.index);
        let entry = index.find_mut(id).ok_or(Error::NotFound)?;
        let backup = entry.clone();
        f(entry);
        if let Err(e) = self.save_index(&index) {
            if let Some(entry) = index.find_mut(id) {
                *entry = backup;
            }
            return Err(e);
        }
        Ok(())
    }

    fn entry_path(&self, id: EntryId) -> PathBuf {
        self.dir.join(FILES_DIR).join(format!("{id}.{FILE_EXT}"))
    }

    fn write_header(&self, password: &str, params: KdfParams) -> Result<()> {
        let header = build_header(&self.master, password, params)?;
        let json = serde_json::to_vec_pretty(&header)
            .map_err(|e| Error::Format(format!("cannot serialize header: {e}")))?;
        write_atomic(&self.dir.join(HEADER_FILE), &json)
    }

    fn index_aad() -> Vec<u8> {
        let mut aad = Vec::with_capacity(5 + INDEX_AAD.len());
        aad.extend_from_slice(INDEX_MAGIC);
        aad.push(INDEX_VERSION);
        aad.extend_from_slice(INDEX_AAD);
        aad
    }

    fn save_index(&self, index: &Index) -> Result<()> {
        let json = Zeroizing::new(
            serde_json::to_vec(index)
                .map_err(|e| Error::Format(format!("cannot serialize index: {e}")))?,
        );
        let sealed = crypto::seal(&self.master, &Self::index_aad(), &json)?;
        let mut out = Vec::with_capacity(5 + sealed.len());
        out.extend_from_slice(INDEX_MAGIC);
        out.push(INDEX_VERSION);
        out.extend_from_slice(&sealed);
        write_atomic(&self.dir.join(INDEX_FILE), &out)
    }

    fn load_index(&self) -> Result<Index> {
        let raw = fs::read(self.dir.join(INDEX_FILE)).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                Error::Corrupt
            } else {
                Error::Io(e)
            }
        })?;
        if raw.len() < 5 || &raw[..4] != INDEX_MAGIC || raw[4] != INDEX_VERSION {
            return Err(Error::Corrupt);
        }
        let json =
            crypto::open(&self.master, &Self::index_aad(), &raw[5..]).ok_or(Error::Corrupt)?;
        serde_json::from_slice(&json).map_err(|_| Error::Corrupt)
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        self.clear_temp_exports();
        // `master` is `Zeroizing` and is wiped when the fields drop.
    }
}

#[cfg(test)]
mod tests;
