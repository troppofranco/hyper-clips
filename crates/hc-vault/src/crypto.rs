//! Low-level cryptographic primitives: randomness, Argon2id key derivation,
//! single-shot AEAD sealing and the chunked file stream format.

use std::io::{self, Read, Write};

use argon2::{Algorithm, Argon2, Version};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Length of every symmetric key (master key and password-derived key).
pub(crate) const KEY_LEN: usize = 32;
/// Length of the Argon2id salt.
pub(crate) const SALT_LEN: usize = 16;
/// XChaCha20-Poly1305 nonce length.
pub(crate) const XNONCE_LEN: usize = 24;
/// Poly1305 tag length.
pub(crate) const TAG_LEN: usize = 16;

/// A 256-bit secret key that is wiped from memory when dropped.
pub(crate) type SecretKey = Zeroizing<[u8; KEY_LEN]>;

/// Fills `buf` from the operating system CSPRNG.
pub(crate) fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf)
        .map_err(|e| Error::Io(io::Error::other(format!("OS random generator failed: {e}"))))
}

/// Returns `N` random bytes from the OS CSPRNG.
pub(crate) fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    fill_random(&mut b)?;
    Ok(b)
}

/// Generates a fresh random 256-bit key.
pub(crate) fn random_key() -> Result<SecretKey> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    fill_random(k.as_mut_slice())?;
    Ok(k)
}

fn cipher(key: &[u8; KEY_LEN]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(key.into())
}

/// Encrypts `msg` with a fresh random nonce; returns `nonce || ciphertext`.
pub(crate) fn seal(key: &[u8; KEY_LEN], aad: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; XNONCE_LEN] = random_array()?;
    let ct = cipher(key)
        .encrypt(&XNonce::from(nonce), Payload { msg, aad })
        .map_err(|_| Error::Format("encryption failed".into()))?;
    let mut out = Vec::with_capacity(XNONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Reverses [`seal`]. Returns `None` if authentication fails or the input is
/// too short.
pub(crate) fn open(key: &[u8; KEY_LEN], aad: &[u8], sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if sealed.len() < XNONCE_LEN + TAG_LEN {
        return None;
    }
    let (nonce, ct) = sealed.split_at(XNONCE_LEN);
    let nonce: [u8; XNONCE_LEN] = nonce.try_into().ok()?;
    cipher(key)
        .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad })
        .ok()
        .map(Zeroizing::new)
}

/// Argon2id cost parameters used to derive the key-encryption key from the
/// password. They are stored in the vault header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_cost_kib: u32,
    /// Number of passes.
    pub t_cost: u32,
    /// Degree of parallelism (lanes).
    pub p_cost: u32,
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes, 1 lane.
    fn default() -> Self {
        Self {
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

impl KdfParams {
    /// Upper bound on memory accepted when unlocking (4 GiB), so a tampered
    /// header cannot make `unlock` allocate arbitrary amounts of memory.
    pub const MAX_M_COST_KIB: u32 = 4 * 1024 * 1024;
    /// Upper bound on passes accepted when unlocking.
    pub const MAX_T_COST: u32 = 64;
    /// Upper bound on lanes accepted when unlocking.
    pub const MAX_P_COST: u32 = 64;

    fn to_argon2(self) -> Result<argon2::Params> {
        if self.m_cost_kib > Self::MAX_M_COST_KIB
            || self.t_cost > Self::MAX_T_COST
            || self.p_cost > Self::MAX_P_COST
        {
            return Err(Error::Format(
                "Argon2 parameters exceed allowed bounds".into(),
            ));
        }
        argon2::Params::new(self.m_cost_kib, self.t_cost, self.p_cost, Some(KEY_LEN))
            .map_err(|e| Error::Format(format!("invalid Argon2 parameters: {e}")))
    }

    /// Checks that the parameters are accepted by Argon2 and within bounds.
    pub fn validate(self) -> Result<()> {
        self.to_argon2().map(|_| ())
    }
}

/// Derives the key-encryption key from `password` with Argon2id v1.3.
pub(crate) fn derive_kek(
    password: &[u8],
    salt: &[u8; SALT_LEN],
    params: KdfParams,
) -> Result<SecretKey> {
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.to_argon2()?);
    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password, salt, out.as_mut_slice())
        .map_err(|e| Error::Format(format!("key derivation failed: {e}")))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Chunked file stream
// ---------------------------------------------------------------------------

/// Magic bytes at the start of every encrypted entry file.
pub(crate) const FILE_MAGIC: &[u8; 4] = b"HCVF";
/// Version of the entry file format.
pub(crate) const FILE_VERSION: u8 = 1;
/// Length of the random per-file nonce prefix.
pub(crate) const NONCE_PREFIX_LEN: usize = 19;
/// Length of the plaintext entry-file header.
pub(crate) const FILE_HEADER_LEN: usize = 4 + 1 + 4 + NONCE_PREFIX_LEN;
/// Default plaintext chunk size (1 MiB).
pub(crate) const DEFAULT_CHUNK_SIZE: u32 = 1024 * 1024;
/// Largest chunk size accepted when decrypting (guards allocations).
const MAX_CHUNK_SIZE: u32 = 64 * 1024 * 1024;

fn chunk_nonce(prefix: &[u8; NONCE_PREFIX_LEN], counter: u32, last: bool) -> XNonce {
    let mut n = [0u8; XNONCE_LEN];
    n[..NONCE_PREFIX_LEN].copy_from_slice(prefix);
    n[NONCE_PREFIX_LEN..NONCE_PREFIX_LEN + 4].copy_from_slice(&counter.to_be_bytes());
    n[XNONCE_LEN - 1] = u8::from(last);
    XNonce::from(n)
}

/// Associated data for every chunk: the file header plus the entry id, so a
/// file cannot be swapped for another entry's file or have its header edited.
fn chunk_aad(header: &[u8; FILE_HEADER_LEN], entry_id: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(FILE_HEADER_LEN + entry_id.len());
    aad.extend_from_slice(header);
    aad.extend_from_slice(entry_id);
    aad
}

/// Reads until `buf` is full or EOF; returns the number of bytes read.
fn read_full<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Streams `src` into `dst` as an encrypted entry file. Returns the number of
/// plaintext bytes consumed.
///
/// Every chunk except the last holds exactly `chunk_size` plaintext bytes;
/// the last one holds fewer (possibly zero), so the reader can tell the final
/// chunk apart from a truncated stream.
pub(crate) fn encrypt_stream<R: Read, W: Write>(
    key: &[u8; KEY_LEN],
    entry_id: &[u8],
    src: &mut R,
    dst: &mut W,
    chunk_size: u32,
) -> Result<u64> {
    let prefix: [u8; NONCE_PREFIX_LEN] = random_array()?;
    let mut header = [0u8; FILE_HEADER_LEN];
    header[..4].copy_from_slice(FILE_MAGIC);
    header[4] = FILE_VERSION;
    header[5..9].copy_from_slice(&chunk_size.to_be_bytes());
    header[9..].copy_from_slice(&prefix);
    dst.write_all(&header)?;

    let aad = chunk_aad(&header, entry_id);
    let cipher = cipher(key);
    let mut buf = vec![0u8; chunk_size as usize];
    let mut counter: u32 = 0;
    let mut total: u64 = 0;
    loop {
        let n = read_full(src, &mut buf)?;
        let last = n < buf.len();
        let ct = cipher
            .encrypt(
                &chunk_nonce(&prefix, counter, last),
                Payload {
                    msg: &buf[..n],
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Format("encryption failed".into()))?;
        dst.write_all(&ct)?;
        total += n as u64;
        if last {
            return Ok(total);
        }
        counter = counter
            .checked_add(1)
            .ok_or_else(|| Error::Format("file too large".into()))?;
    }
}

/// Streams an encrypted entry file from `src` into `dst`, verifying every
/// chunk. Returns the number of plaintext bytes written. Any authentication
/// or structural failure (including truncation) yields [`Error::Corrupt`];
/// plaintext already written to `dst` must then be discarded by the caller.
pub(crate) fn decrypt_stream<R: Read, W: Write>(
    key: &[u8; KEY_LEN],
    entry_id: &[u8],
    src: &mut R,
    dst: &mut W,
) -> Result<u64> {
    let mut header = [0u8; FILE_HEADER_LEN];
    if read_full(src, &mut header)? != FILE_HEADER_LEN
        || &header[..4] != FILE_MAGIC
        || header[4] != FILE_VERSION
    {
        return Err(Error::Corrupt);
    }
    let chunk_size = u32::from_be_bytes([header[5], header[6], header[7], header[8]]);
    if chunk_size == 0 || chunk_size > MAX_CHUNK_SIZE {
        return Err(Error::Corrupt);
    }
    let mut prefix = [0u8; NONCE_PREFIX_LEN];
    prefix.copy_from_slice(&header[9..]);

    let aad = chunk_aad(&header, entry_id);
    let cipher = cipher(key);
    let mut buf = vec![0u8; chunk_size as usize + TAG_LEN];
    let mut counter: u32 = 0;
    let mut total: u64 = 0;
    loop {
        let n = read_full(src, &mut buf)?;
        let last = n < buf.len();
        if n < TAG_LEN {
            // Includes n == 0: the stream ended without a final chunk.
            return Err(Error::Corrupt);
        }
        let pt = cipher
            .decrypt(
                &chunk_nonce(&prefix, counter, last),
                Payload {
                    msg: &buf[..n],
                    aad: &aad,
                },
            )
            .map_err(|_| Error::Corrupt)?;
        dst.write_all(&pt)?;
        total += pt.len() as u64;
        if last {
            return Ok(total);
        }
        counter = counter.checked_add(1).ok_or(Error::Corrupt)?;
    }
}

/// Lower-case hex encoding.
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

/// Hex decoding (either case). Returns `None` on invalid input.
pub(crate) fn from_hex(s: &str) -> Option<Vec<u8>> {
    fn nib(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let b = s.as_bytes();
    let (pairs, rest) = b.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|&[hi, lo]| Some((nib(hi)? << 4) | nib(lo)?))
        .collect()
}
