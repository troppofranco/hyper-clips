#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn limits() -> PluginLimits {
    PluginLimits {
        fuel_per_call: 1_000_000,
        max_memory_bytes: 1024 * 1024, // 16 pages
        ..PluginLimits::default()
    }
}

fn manifest(id: &str, permissions: &[Permission]) -> PluginManifest {
    PluginManifest {
        id: id.to_owned(),
        name: id.to_owned(),
        version: "1.0.0".to_owned(),
        description: String::new(),
        author: String::new(),
        api: API_VERSION,
        permissions: permissions.to_vec(),
    }
}

fn event() -> ClipEvent {
    ClipEvent {
        path: PathBuf::from("C:/clips/a.mp4"),
        game: Some("Valorant".to_owned()),
        duration_s: 30.0,
        kind: ClipKind::Clip,
        markers: vec![1.5, 10.0],
    }
}

fn load(host: &mut PluginHost, id: &str, perms: &[Permission], wat: &str) -> PluginId {
    let wasm = wat::parse_str(wat).unwrap();
    host.load_bytes(manifest(id, perms), PathBuf::new(), &wasm)
        .unwrap()
}

/// Finds `"game":"` in the input, adds the following string as a tag and logs.
/// Logs "denied" if add_tag returned -1.
const TAGGER: &str = r#"
(module
  (import "hc" "log" (func $log (param i32 i32) (result i32)))
  (import "hc" "add_tag" (func $add_tag (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "\"game\":\"")
  (data (i32.const 16) "tagger ran")
  (data (i32.const 32) "denied")
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "hc_on_clip_saved") (param $ptr i32) (param $len i32)
    (local $i i32) (local $end i32) (local $j i32)
    (drop (call $log (i32.const 16) (i32.const 10)))
    (local.set $i (local.get $ptr))
    (local.set $end (i32.sub (i32.add (local.get $ptr) (local.get $len)) (i32.const 8)))
    (block $done
      (loop $scan
        (br_if $done (i32.gt_s (local.get $i) (local.get $end)))
        (if (i64.eq (i64.load (local.get $i)) (i64.load (i32.const 0)))
          (then
            (local.set $i (i32.add (local.get $i) (i32.const 8)))
            (local.set $j (local.get $i))
            (block $q
              (loop $find
                (br_if $q (i32.eq (i32.load8_u (local.get $j)) (i32.const 34)))
                (local.set $j (i32.add (local.get $j) (i32.const 1)))
                (br $find)))
            (if (i32.eq
                  (call $add_tag (local.get $i) (i32.sub (local.get $j) (local.get $i)))
                  (i32.const -1))
              (then (drop (call $log (i32.const 32) (i32.const 6)))))
            (br $done)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $scan))))
)
"#;

const LOOPER: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 0))
  (func (export "hc_on_clip_saved") (param i32 i32)
    (loop $forever (br $forever)))
)
"#;

#[test]
fn tagger_adds_tag_from_input_and_logs() {
    let mut host = PluginHost::new(limits());
    let id = load(&mut host, "tagger", &[Permission::Tags], TAGGER);
    let results = host.on_clip_saved(&event());
    assert_eq!(results.len(), 1);
    let (got_id, out) = &results[0];
    assert_eq!(got_id, &id);
    let out = out.as_ref().unwrap();
    assert_eq!(out.tags, vec!["Valorant".to_owned()]);
    assert_eq!(out.logs, vec!["tagger ran".to_owned()]);
    assert!(out.notifications.is_empty());
    // Output does not leak between calls.
    let again = host.on_clip_saved(&event());
    assert_eq!(again[0].1.as_ref().unwrap().tags.len(), 1);
    assert!(host.plugins()[0].is_enabled());
}

#[test]
fn tags_require_permission() {
    let mut host = PluginHost::new(limits());
    load(&mut host, "tagger", &[Permission::Notify], TAGGER);
    let results = host.on_clip_saved(&event());
    let out = results[0].1.as_ref().unwrap();
    assert!(out.tags.is_empty());
    assert_eq!(out.logs, vec!["tagger ran".to_owned(), "denied".to_owned()]);
    assert!(host.plugins()[0].is_enabled());
}

#[test]
fn notify_requires_permission() {
    let wat = r#"
(module
  (import "hc" "notify" (func $notify (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "Clip saved!")
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "hc_on_clip_saved") (param i32 i32)
    (drop (call $notify (i32.const 0) (i32.const 11))))
)"#;
    let mut host = PluginHost::new(limits());
    load(&mut host, "with", &[Permission::Notify], wat);
    load(&mut host, "without", &[], wat);
    let results = host.on_clip_saved(&event());
    assert_eq!(
        results[0].1.as_ref().unwrap().notifications,
        vec!["Clip saved!".to_owned()]
    );
    assert!(results[1].1.as_ref().unwrap().notifications.is_empty());
}

#[test]
fn infinite_loop_runs_out_of_fuel_and_disables_plugin() {
    let mut host = PluginHost::new(limits());
    load(&mut host, "looper", &[], LOOPER);
    load(&mut host, "tagger", &[Permission::Tags], TAGGER);

    let results = host.on_clip_saved(&event());
    assert_eq!(results.len(), 2);
    assert!(matches!(
        results[0].1,
        Err(PluginError::OutOfFuel(1_000_000))
    ));
    assert!(results[1].1.is_ok());

    let looper = &host.plugins()[0];
    assert!(matches!(looper.status, PluginStatus::Disabled { .. }));

    // The host survives; the disabled plugin is skipped from now on.
    let results = host.on_clip_saved(&event());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.as_str(), "tagger");
}

#[test]
fn start_function_out_of_fuel_is_rejected() {
    let wat = r#"
(module
  (memory (export "memory") 1)
  (func $start (loop $l (br $l)))
  (start $start)
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 0))
)"#;
    let mut host = PluginHost::new(limits());
    let err = host
        .load_bytes(
            manifest("starter", &[]),
            PathBuf::new(),
            &wat::parse_str(wat).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(err, PluginError::OutOfFuel(_)), "{err}");
    assert!(host.plugins().is_empty());
}

#[test]
fn wasi_import_is_rejected() {
    let wat = r#"
(module
  (import "wasi_snapshot_preview1" "fd_write"
    (func (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 0))
)"#;
    let mut host = PluginHost::new(limits());
    let err = host
        .load_bytes(
            manifest("wasi", &[]),
            PathBuf::new(),
            &wat::parse_str(wat).unwrap(),
        )
        .unwrap_err();
    match err {
        PluginError::ForbiddenImport { module, name } => {
            assert_eq!(module, "wasi_snapshot_preview1");
            assert_eq!(name, "fd_write");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn unknown_hc_import_and_bad_signature_are_rejected() {
    let unknown = r#"(module (import "hc" "open_file" (func (param i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 0)))"#;
    let bad_sig = r#"(module (import "hc" "log" (func (param i32)))
      (memory (export "memory") 1)
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 0)))"#;
    let imported_memory = r#"(module (import "hc" "log" (memory 1))
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 0)))"#;
    let mut host = PluginHost::new(limits());
    let e1 = host
        .load_bytes(
            manifest("a", &[]),
            PathBuf::new(),
            &wat::parse_str(unknown).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(e1, PluginError::ForbiddenImport { .. }), "{e1}");
    let e2 = host
        .load_bytes(
            manifest("b", &[]),
            PathBuf::new(),
            &wat::parse_str(bad_sig).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(e2, PluginError::InvalidModule(_)), "{e2}");
    let e3 = host
        .load_bytes(
            manifest("c", &[]),
            PathBuf::new(),
            &wat::parse_str(imported_memory).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(e3, PluginError::ForbiddenImport { .. }), "{e3}");
}

#[test]
fn missing_exports_are_rejected() {
    let no_alloc = r#"(module (memory (export "memory") 1))"#;
    let no_memory = r#"(module (func (export "hc_alloc") (param i32) (result i32) (i32.const 0)))"#;
    let bad_handler = r#"(module (memory (export "memory") 1)
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 0))
      (func (export "hc_on_clip_saved") (param i32)))"#;
    let mut host = PluginHost::new(limits());
    for (id, wat) in [("a", no_alloc), ("b", no_memory), ("c", bad_handler)] {
        let err = host
            .load_bytes(
                manifest(id, &[]),
                PathBuf::new(),
                &wat::parse_str(wat).unwrap(),
            )
            .unwrap_err();
        assert!(matches!(err, PluginError::BadExport { .. }), "{id}: {err}");
    }
}

#[test]
fn memory_growth_beyond_limit_fails() {
    // Grows by 100 pages (6.4 MiB) with a 1 MiB limit; logs the outcome.
    let wat = r#"
(module
  (import "hc" "log" (func $log (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 0) "grow failed")
  (data (i32.const 16) "grow ok")
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "hc_on_clip_saved") (param i32 i32)
    (if (i32.eq (memory.grow (i32.const 100)) (i32.const -1))
      (then (drop (call $log (i32.const 0) (i32.const 11))))
      (else (drop (call $log (i32.const 16) (i32.const 7))))))
)"#;
    let mut host = PluginHost::new(limits());
    load(&mut host, "grower", &[], wat);
    let results = host.on_clip_saved(&event());
    assert_eq!(
        results[0].1.as_ref().unwrap().logs,
        vec!["grow failed".to_owned()]
    );

    // Declaring an initial memory over the limit fails at load time.
    let big = r#"(module (memory (export "memory") 100)
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 0)))"#;
    let err = host
        .load_bytes(
            manifest("big", &[]),
            PathBuf::new(),
            &wat::parse_str(big).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(err, PluginError::Instantiation(_)), "{err}");
}

#[test]
fn out_of_bounds_pointer_traps_and_disables() {
    let wat = r#"
(module
  (import "hc" "log" (func $log (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 0))
  (func (export "hc_on_clip_saved") (param i32 i32)
    (drop (call $log (i32.const 65530) (i32.const 100))))
)"#;
    let mut host = PluginHost::new(limits());
    load(&mut host, "oob", &[], wat);
    let results = host.on_clip_saved(&event());
    assert!(matches!(results[0].1, Err(PluginError::Trap(_))));
    assert!(!host.plugins()[0].is_enabled());
}

#[test]
fn bad_alloc_pointer_disables() {
    let wat = r#"(module (memory (export "memory") 1)
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 65535))
      (func (export "hc_on_clip_saved") (param i32 i32)))"#;
    let mut host = PluginHost::new(limits());
    load(&mut host, "badalloc", &[], wat);
    let results = host.on_clip_saved(&event());
    assert!(matches!(results[0].1, Err(PluginError::Trap(_))));
    assert!(!host.plugins()[0].is_enabled());
}

#[test]
fn log_is_truncated_and_rate_limited() {
    let wat = r#"
(module
  (import "hc" "log" (func $log (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 8192))
  (func (export "hc_on_clip_saved") (param i32 i32)
    (local $n i32)
    (loop $l
      (drop (call $log (i32.const 0) (i32.const 4000)))
      (local.set $n (i32.add (local.get $n) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $n) (i32.const 100)))))
)"#;
    let mut host = PluginHost::new(limits());
    load(&mut host, "chatty", &[], wat);
    let results = host.on_clip_saved(&event());
    let out = results[0].1.as_ref().unwrap();
    assert_eq!(out.logs.len(), PluginLimits::default().max_logs_per_call);
    assert!(out.logs.iter().all(|l| l.len() <= 1024));
}

#[test]
fn plugins_without_handler_are_skipped_and_unload_works() {
    let wat = r#"(module (memory (export "memory") 1)
      (func (export "hc_alloc") (param i32) (result i32) (i32.const 0)))"#;
    let mut host = PluginHost::new(limits());
    let id = load(&mut host, "idle", &[], wat);
    assert!(host.on_clip_saved(&event()).is_empty());
    assert!(matches!(
        host.load_bytes(
            manifest("idle", &[]),
            PathBuf::new(),
            &wat::parse_str(wat).unwrap()
        ),
        Err(PluginError::AlreadyLoaded(_))
    ));
    host.unload(&id).unwrap();
    assert!(host.plugins().is_empty());
    assert!(matches!(host.unload(&id), Err(PluginError::NotFound(_))));
}

#[test]
fn wasm_size_limit() {
    let mut host = PluginHost::new(PluginLimits {
        max_wasm_bytes: 8,
        ..limits()
    });
    let err = host
        .load_bytes(
            manifest("x", &[]),
            PathBuf::new(),
            &wat::parse_str(LOOPER).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(err, PluginError::WasmTooLarge { .. }));
}

#[test]
fn event_json_shape() {
    let json: serde_json::Value = serde_json::from_str(&event().to_json()).unwrap();
    assert_eq!(json["path"], "C:/clips/a.mp4");
    assert_eq!(json["game"], "Valorant");
    assert_eq!(json["duration_s"], 30.0);
    assert_eq!(json["kind"], "clip");
    assert_eq!(json["markers"][1], 10.0);
    let shot = ClipEvent {
        game: None,
        kind: ClipKind::Screenshot,
        ..event()
    };
    let json: serde_json::Value = serde_json::from_str(&shot.to_json()).unwrap();
    assert!(json["game"].is_null());
    assert_eq!(json["kind"], "screenshot");
}

#[test]
fn manifest_parsing() {
    let ok = PluginManifest::from_toml_str(
        r#"
id = "auto-tagger"
name = "Auto Tagger"
version = "1.0.0"
description = "Tags clips"
author = "Someone"
api = 1
permissions = ["tags", "notify"]
future_key = true
"#,
    )
    .unwrap();
    assert_eq!(ok.id, "auto-tagger");
    assert!(ok.has_permission(Permission::Tags));
    assert!(ok.has_permission(Permission::Notify));

    let minimal =
        PluginManifest::from_toml_str("id = \"a\"\nname = \"A\"\nversion = \"0.1\"\napi = 1\n")
            .unwrap();
    assert!(minimal.permissions.is_empty());

    let base = "name = \"A\"\nversion = \"1\"\n";
    for bad in [
        format!("id = \"a\"\n{base}api = 1\npermissions = [\"filesystem\"]\n"),
        format!("id = \"Bad Id\"\n{base}api = 1\n"),
        format!("id = \"../evil\"\n{base}api = 1\n"),
        format!("id = \"a\"\n{base}"),
    ] {
        assert!(
            matches!(
                PluginManifest::from_toml_str(&bad),
                Err(PluginError::Manifest(_))
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        PluginManifest::from_toml_str(&format!("id = \"a\"\n{base}api = 2\n")),
        Err(PluginError::UnsupportedApi(2))
    ));
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hc-plugins-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_plugin(root: &Path, folder: &str, toml: &str, wasm: Option<&[u8]>) {
    let dir = root.join(folder);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(MANIFEST_FILE), toml).unwrap();
    if let Some(wasm) = wasm {
        fs::write(dir.join(WASM_FILE), wasm).unwrap();
    }
}

#[test]
fn discovery_and_load_from_disk() {
    let tmp = TempDir::new("discover");
    let wasm = wat::parse_str(TAGGER).unwrap();
    let toml = |id: &str| {
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"1\"\napi = 1\npermissions = [\"tags\"]\n"
        )
    };
    write_plugin(&tmp.0, "good", &toml("good"), Some(&wasm));
    write_plugin(&tmp.0, "mismatch", &toml("other"), Some(&wasm));
    write_plugin(&tmp.0, "nowasm", &toml("nowasm"), None);
    fs::create_dir_all(tmp.0.join("not-a-plugin")).unwrap();

    let found = discover(&tmp.0);
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].as_ref().unwrap().id, "good");
    assert!(matches!(&found[1], Err((p, PluginError::Manifest(_))) if p.ends_with("mismatch")));
    assert!(matches!(&found[2], Err((p, PluginError::Io { .. })) if p.ends_with("nowasm")));

    assert!(discover(&tmp.0.join("missing")).is_empty());

    let mut host = PluginHost::new(limits());
    let loaded = host.load_all(&tmp.0);
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].as_ref().unwrap().as_str(), "good");
    assert_eq!(host.plugins().len(), 1);
    assert_eq!(host.plugins()[0].dir, tmp.0.join("good"));
    let results = host.on_clip_saved(&event());
    assert_eq!(
        results[0].1.as_ref().unwrap().tags,
        vec!["Valorant".to_owned()]
    );
}

#[test]
fn host_call_from_start_function_fails_cleanly() {
    let wat = r#"
(module
  (import "hc" "log" (func $log (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func $start (drop (call $log (i32.const 0) (i32.const 1))))
  (start $start)
  (func (export "hc_alloc") (param i32) (result i32) (i32.const 0))
)"#;
    let mut host = PluginHost::new(limits());
    let err = host
        .load_bytes(
            manifest("s", &[]),
            PathBuf::new(),
            &wat::parse_str(wat).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(err, PluginError::Instantiation(_)), "{err}");
}
