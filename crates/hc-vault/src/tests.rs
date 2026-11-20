#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

const FAST: KdfParams = KdfParams {
    m_cost_kib: 64,
    t_cost: 1,
    p_cost: 1,
};
const PW: &str = "correct horse battery staple";

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    vault_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let vault_dir = root.join("vault");
    Fixture {
        _tmp: tmp,
        root,
        vault_dir,
    }
}

fn write_src(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, data).unwrap();
    p
}

fn roundtrip(len: usize) {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let data = pseudo_random(len, len as u64 + 7);
    let src = write_src(&f.root, "clip.mp4", &data);
    let id = v.add_file(&src, false).unwrap();
    assert!(src.exists());
    let e = v.get(id).unwrap();
    assert_eq!(e.size, len as u64);
    assert_eq!(e.name, "clip.mp4");
    let out = f.root.join("out.mp4");
    v.extract(id, &out).unwrap();
    assert_eq!(fs::read(&out).unwrap(), data);
    v.verify(id).unwrap();
}

#[test]
fn roundtrip_small() {
    roundtrip(1234);
}

#[test]
fn roundtrip_empty() {
    roundtrip(0);
}

#[test]
fn roundtrip_exact_chunk_multiple() {
    roundtrip(2 * crypto::DEFAULT_CHUNK_SIZE as usize);
}

#[test]
fn roundtrip_multi_chunk() {
    roundtrip(5 * 1024 * 1024 / 2);
}

#[test]
fn unlock_persists_entries_and_wrong_password_fails() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let data = pseudo_random(4096, 1);
    let id = v
        .add_file_with_metadata(
            &write_src(&f.root, "a.mp4", &data),
            false,
            serde_json::json!({"game": "x"}),
        )
        .unwrap();
    v.lock();

    assert!(Vault::exists(&f.vault_dir));
    assert!(matches!(
        Vault::unlock(&f.vault_dir, "wrong"),
        Err(Error::WrongPassword)
    ));
    let v = Vault::unlock(&f.vault_dir, PW).unwrap();
    assert_eq!(v.kdf_params(), FAST);
    let list = v.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].metadata["game"], "x");
}

#[test]
fn create_twice_fails() {
    let f = fixture();
    let _v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    assert!(matches!(
        Vault::create_with_params(&f.vault_dir, PW, FAST),
        Err(Error::AlreadyExists)
    ));
}

#[test]
fn default_params() {
    let p = KdfParams::default();
    assert_eq!((p.m_cost_kib, p.t_cost, p.p_cost), (65536, 3, 1));
}

fn add_big(f: &Fixture, v: &Vault) -> (EntryId, Vec<u8>) {
    let data = pseudo_random(5 * 1024 * 1024 / 2, 99);
    let id = v
        .add_file(&write_src(&f.root, "big.mp4", &data), false)
        .unwrap();
    (id, data)
}

fn assert_no_partials(dir: &Path) {
    for e in fs::read_dir(dir).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!name.contains("hcv-part"), "partial output left: {name}");
    }
}

#[test]
fn tamper_is_detected_and_partial_removed() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let (id, _) = add_big(&f, &v);
    let path = v.entry_path(id);
    let mut bytes = fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    fs::write(&path, &bytes).unwrap();

    let out = f.root.join("out.mp4");
    assert!(matches!(v.extract(id, &out), Err(Error::Corrupt)));
    assert!(!out.exists());
    assert_no_partials(&f.root);
    assert!(matches!(v.verify(id), Err(Error::Corrupt)));
}

#[test]
fn header_tamper_is_detected() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let (id, _) = add_big(&f, &v);
    let path = v.entry_path(id);
    let mut bytes = fs::read(&path).unwrap();
    bytes[10] ^= 0x80; // nonce prefix
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(v.verify(id), Err(Error::Corrupt)));
}

#[test]
fn truncation_is_detected() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let (id, data) = add_big(&f, &v);
    let path = v.entry_path(id);
    let bytes = fs::read(&path).unwrap();
    let chunk = crypto::DEFAULT_CHUNK_SIZE as usize;
    let last_ct = data.len() % chunk + crypto::TAG_LEN;

    // Drop the whole last chunk: remaining stream ends on a full chunk.
    fs::write(&path, &bytes[..bytes.len() - last_ct]).unwrap();
    let out = f.root.join("out.mp4");
    assert!(matches!(v.extract(id, &out), Err(Error::Corrupt)));
    assert!(!out.exists());
    assert_no_partials(&f.root);

    // Cut a few bytes off the end.
    fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
    assert!(matches!(v.extract(id, &out), Err(Error::Corrupt)));

    // Header only.
    fs::write(&path, &bytes[..crypto::FILE_HEADER_LEN]).unwrap();
    assert!(matches!(v.verify(id), Err(Error::Corrupt)));
}

#[test]
fn chunk_reorder_is_detected() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let (id, _) = add_big(&f, &v);
    let path = v.entry_path(id);
    let bytes = fs::read(&path).unwrap();
    let h = crypto::FILE_HEADER_LEN;
    let c = crypto::DEFAULT_CHUNK_SIZE as usize + crypto::TAG_LEN;
    let mut swapped = bytes[..h].to_vec();
    swapped.extend_from_slice(&bytes[h + c..h + 2 * c]);
    swapped.extend_from_slice(&bytes[h..h + c]);
    swapped.extend_from_slice(&bytes[h + 2 * c..]);
    fs::write(&path, &swapped).unwrap();
    assert!(matches!(v.verify(id), Err(Error::Corrupt)));
}

#[test]
fn swapping_files_between_entries_is_detected() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let data = pseudo_random(1000, 3);
    let a = v
        .add_file(&write_src(&f.root, "a.mp4", &data), false)
        .unwrap();
    let b = v
        .add_file(&write_src(&f.root, "b.mp4", &data), false)
        .unwrap();
    fs::copy(v.entry_path(a), v.entry_path(b)).unwrap();
    assert!(matches!(v.verify(b), Err(Error::Corrupt)));
}

#[test]
fn password_change_keeps_data() {
    let f = fixture();
    let mut v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let data = pseudo_random(300_000, 5);
    let id = v
        .add_file(&write_src(&f.root, "a.mp4", &data), false)
        .unwrap();
    v.change_password("new password").unwrap();
    v.lock();

    assert!(matches!(
        Vault::unlock(&f.vault_dir, PW),
        Err(Error::WrongPassword)
    ));
    let v = Vault::unlock(&f.vault_dir, "new password").unwrap();
    let out = f.root.join("out.mp4");
    v.extract(id, &out).unwrap();
    assert_eq!(fs::read(&out).unwrap(), data);
}

#[test]
fn tampered_kdf_params_fail_unlock() {
    let f = fixture();
    Vault::create_with_params(&f.vault_dir, PW, FAST)
        .unwrap()
        .lock();
    let hp = f.vault_dir.join(HEADER_FILE);
    let mut h: serde_json::Value = serde_json::from_slice(&fs::read(&hp).unwrap()).unwrap();
    h["kdf"]["t_cost"] = serde_json::json!(2);
    fs::write(&hp, serde_json::to_vec(&h).unwrap()).unwrap();
    assert!(matches!(
        Vault::unlock(&f.vault_dir, PW),
        Err(Error::WrongPassword)
    ));
    h["kdf"]["m_cost_kib"] = serde_json::json!(u32::MAX);
    fs::write(&hp, serde_json::to_vec(&h).unwrap()).unwrap();
    assert!(matches!(
        Vault::unlock(&f.vault_dir, PW),
        Err(Error::Format(_))
    ));
}

fn collect_bytes(dir: &Path, out: &mut Vec<u8>) {
    for e in fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        out.extend_from_slice(p.file_name().unwrap().to_string_lossy().as_bytes());
        if p.is_dir() {
            collect_bytes(&p, out);
        } else {
            out.extend_from_slice(&fs::read(&p).unwrap());
        }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn index_hides_file_names() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let name = "SecretGameplay_ZebraUnicorn.mp4";
    let id = v
        .add_file_with_metadata(
            &write_src(&f.root, name, b"hello"),
            true,
            serde_json::json!({"tag": "PrivateTagValue"}),
        )
        .unwrap();
    assert!(!f.root.join(name).exists(), "original should be deleted");
    v.rename(id, "RenamedSecretName.mp4").unwrap();

    let mut all = Vec::new();
    collect_bytes(&f.vault_dir, &mut all);
    for needle in [
        "SecretGameplay",
        "ZebraUnicorn",
        "RenamedSecretName",
        "PrivateTagValue",
        "hello",
    ] {
        assert!(!contains(&all, needle.as_bytes()), "{needle} leaked");
    }
    drop(v);
    let v = Vault::unlock(&f.vault_dir, PW).unwrap();
    assert_eq!(v.get(id).unwrap().name, "RenamedSecretName.mp4");
}

#[test]
fn remove_and_not_found() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let id = v
        .add_file(&write_src(&f.root, "a.mp4", b"x"), false)
        .unwrap();
    let path = v.entry_path(id);
    assert!(path.exists());
    v.remove(id).unwrap();
    assert!(!path.exists());
    assert!(v.list().is_empty());
    assert!(matches!(v.remove(id), Err(Error::NotFound)));
    assert!(matches!(
        v.extract(id, &f.root.join("o")),
        Err(Error::NotFound)
    ));
    assert!(matches!(v.rename(id, "x"), Err(Error::NotFound)));
    assert!("../../etc".parse::<EntryId>().is_err());
}

#[test]
fn export_temp_is_cleaned_on_lock() {
    let f = fixture();
    let v = Vault::create_with_params(&f.vault_dir, PW, FAST).unwrap();
    let data = pseudo_random(2048, 11);
    let id = v
        .add_file(&write_src(&f.root, "play me.mp4", &data), false)
        .unwrap();
    let p = v.export_temp(id).unwrap();
    assert_eq!(p.file_name().unwrap(), "play me.mp4");
    assert_eq!(fs::read(&p).unwrap(), data);
    let parent = p.parent().unwrap().to_path_buf();
    v.lock();
    assert!(!parent.exists());
}

#[test]
fn entry_id_roundtrips() {
    let id = EntryId::random().unwrap();
    let s = id.to_string();
    assert_eq!(s.len(), 32);
    assert_eq!(s.parse::<EntryId>().unwrap(), id);
}

#[test]
fn sanitize_names() {
    assert_eq!(sanitize_file_name("..\\..\\evil.mp4"), "evil.mp4");
    assert_eq!(sanitize_file_name("a:b?.mp4"), "a_b_.mp4");
    assert_eq!(sanitize_file_name(".."), "clip");
}
