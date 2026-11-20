//! Sandboxed WebAssembly plugin host for Hyper Clips.
//!
//! Plugins are WebAssembly modules executed by [wasmi], a pure-Rust
//! **interpreter** (no JIT, no native code generation). Each plugin lives in
//! its own folder:
//!
//! ```text
//! <plugins dir>/<id>/plugin.toml   # see PluginManifest
//! <plugins dir>/<id>/plugin.wasm   # the module, at most PluginLimits::max_wasm_bytes
//! ```
//!
//! # Sandbox and guarantees
//!
//! * **No ambient capabilities.** There is no WASI and no other import
//!   namespace: a module may import only the three host functions of ABI v1
//!   (module `"hc"`). Any other import (e.g. `wasi_snapshot_preview1.fd_write`)
//!   is rejected at load time with [`PluginError::ForbiddenImport`], before
//!   instantiation. Consequently a plugin has no filesystem, network, clock,
//!   randomness, threads or process access; it can only compute on the event
//!   JSON it is handed and emit tags / notifications / log lines.
//! * **CPU is bounded by fuel.** Instantiation (including a module `start`
//!   function) and each event delivery (`hc_alloc` plus the handler) each get
//!   [`PluginLimits::fuel_per_call`] fuel.
//!   Running out traps with [`PluginError::OutOfFuel`].
//! * **Memory is bounded.** Linear memory (initial size and growth) is capped
//!   by [`PluginLimits::max_memory_bytes`] through a store resource limiter;
//!   `memory.grow` beyond it returns `-1` as the spec requires. One memory,
//!   one table and one instance per plugin.
//! * **Output is bounded.** Tags, notifications and log lines are size- and
//!   count-limited per call (see [`PluginLimits`]).
//! * **Failures are contained.** A trap, out-of-fuel or ABI violation disables
//!   the plugin for the rest of the session ([`PluginStatus::Disabled`]) and is
//!   reported as an error; the host never panics because of plugin behaviour.
//!
//! What is **not** guaranteed: a malicious plugin can still burn CPU up to the
//! fuel limit on every event until it is disabled, can allocate up to the
//! memory limit, and can return misleading tags/notifications within its
//! granted permissions. Calls run synchronously on the caller's thread, so
//! run [`PluginHost::on_clip_saved`] off any latency-sensitive thread.
//! Interpreter bugs in wasmi itself are outside this crate's control.
//!
//! # ABI v1
//!
//! All pointers/lengths are `i32` offsets/byte counts into the plugin's
//! exported `memory`. Strings are UTF-8, not NUL-terminated.
//!
//! Plugin **exports**:
//!
//! | export                                   | required | purpose |
//! |------------------------------------------|----------|---------|
//! | `memory`                                 | yes      | linear memory |
//! | `hc_alloc(len: i32) -> i32`              | yes      | return a pointer to `len` writable bytes; the host writes event input there |
//! | `hc_on_clip_saved(ptr: i32, len: i32)`   | no       | called after a clip / recording / screenshot is saved |
//!
//! `hc_on_clip_saved` receives the JSON produced from [`ClipEvent`]:
//! `{"path": "...", "game": "..."|null, "duration_s": 12.5,
//! "kind": "clip"|"recording"|"screenshot", "markers": [3.2, 7.9]}`
//! (`markers` are seconds from the start of the media). The buffer returned
//! by `hc_alloc` belongs to the plugin; the host never frees it.
//!
//! Host **imports** (module `"hc"`, all `(ptr: i32, len: i32) -> i32`):
//!
//! | import    | permission | effect |
//! |-----------|------------|--------|
//! | `log`     | none       | log line (truncated to [`PluginLimits::max_log_bytes`], at most [`PluginLimits::max_logs_per_call`] per call) |
//! | `add_tag` | `"tags"`   | adds a tag to [`PluginOutput::tags`] (trimmed; empty/duplicates dropped) |
//! | `notify`  | `"notify"` | adds a message to [`PluginOutput::notifications`] |
//!
//! Return codes: [`RC_OK`] (0) accepted, [`RC_DENIED`] (-1) permission not
//! granted (call ignored, the plugin keeps running), [`RC_LIMIT`] (-2) a size
//! or count limit was exceeded (dropped), [`RC_INVALID_UTF8`] (-3). A
//! `ptr`/`len` outside linear memory or negative is an ABI violation and
//! **traps**, disabling the plugin. Host functions are not available while
//! the module's `start` function runs (calling one fails instantiation); do
//! one-time setup lazily in the first handler call instead.
//!
//! A plugin instance persists across events (globals and memory survive
//! between calls) until it is unloaded or disabled.

mod manifest;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use wasmi::errors::{ErrorKind, FuelError};
use wasmi::{
    Caller, CompilationMode, Config, Engine, ExternType, Instance, Linker, Memory, Module, Store,
    StoreLimits, StoreLimitsBuilder, TrapCode, TypedFunc, ValType,
};

pub use manifest::{Permission, PluginManifest, discover};

/// The only plugin ABI version this host implements.
pub const API_VERSION: u32 = 1;
/// Manifest file name inside a plugin folder.
pub const MANIFEST_FILE: &str = "plugin.toml";
/// Module file name inside a plugin folder.
pub const WASM_FILE: &str = "plugin.wasm";
/// Import module name for host functions.
pub const HOST_MODULE: &str = "hc";

/// Host function return code: accepted.
pub const RC_OK: i32 = 0;
/// Host function return code: the plugin lacks the required permission.
pub const RC_DENIED: i32 = -1;
/// Host function return code: a size/count limit was exceeded; dropped.
pub const RC_LIMIT: i32 = -2;
/// Host function return code: the string was not valid UTF-8.
pub const RC_INVALID_UTF8: i32 = -3;

const HOST_FUNCS: [&str; 3] = ["log", "add_tag", "notify"];

/// Resource limits applied to every plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLimits {
    /// Fuel (roughly: executed instructions) per call into the plugin.
    pub fuel_per_call: u64,
    /// Maximum linear memory size in bytes.
    pub max_memory_bytes: usize,
    /// Maximum size of `plugin.wasm` in bytes.
    pub max_wasm_bytes: u64,
    /// Maximum size of the event JSON handed to a plugin.
    pub max_input_bytes: usize,
    /// Log lines longer than this are truncated.
    pub max_log_bytes: usize,
    /// Log lines accepted per call; further calls return [`RC_LIMIT`].
    pub max_logs_per_call: usize,
    /// Maximum tag length in bytes (after trimming).
    pub max_tag_bytes: usize,
    /// Tags accepted per call.
    pub max_tags_per_call: usize,
    /// Maximum notification length in bytes.
    pub max_notification_bytes: usize,
    /// Notifications accepted per call.
    pub max_notifications_per_call: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            fuel_per_call: 50_000_000,
            max_memory_bytes: 32 * 1024 * 1024,
            max_wasm_bytes: 4 * 1024 * 1024,
            max_input_bytes: 256 * 1024,
            max_log_bytes: 1024,
            max_logs_per_call: 32,
            max_tag_bytes: 64,
            max_tags_per_call: 16,
            max_notification_bytes: 512,
            max_notifications_per_call: 4,
        }
    }
}

/// Errors from loading or calling plugins.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// Filesystem error.
    #[error("{path}: {source}")]
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Invalid `plugin.toml`.
    #[error("invalid manifest: {0}")]
    Manifest(String),
    /// Manifest requests an ABI version this host does not implement.
    #[error("unsupported plugin api version {0} (supported: {API_VERSION})")]
    UnsupportedApi(u32),
    /// `plugin.wasm` exceeds [`PluginLimits::max_wasm_bytes`].
    #[error("plugin.wasm is {size} bytes, limit is {max}")]
    WasmTooLarge {
        /// Actual size.
        size: u64,
        /// Limit.
        max: u64,
    },
    /// The module is not valid WebAssembly or uses unsupported features.
    #[error("invalid wasm module: {0}")]
    InvalidModule(String),
    /// The module imports something other than the `hc` host functions.
    #[error(
        "forbidden import {module}.{name}: plugins may only import hc.log, hc.add_tag, hc.notify"
    )]
    ForbiddenImport {
        /// Import module.
        module: String,
        /// Import name.
        name: String,
    },
    /// A required export is missing or has the wrong type.
    #[error("bad export {name}: {reason}")]
    BadExport {
        /// Export name.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// Instantiation failed (e.g. initial memory over the limit).
    #[error("instantiation failed: {0}")]
    Instantiation(String),
    /// A plugin with this id is already loaded.
    #[error("plugin {0} is already loaded")]
    AlreadyLoaded(String),
    /// No plugin with this id is loaded.
    #[error("plugin {0} is not loaded")]
    NotFound(String),
    /// The plugin ran out of fuel (CPU budget).
    #[error("plugin ran out of fuel (limit {0} per call)")]
    OutOfFuel(u64),
    /// The plugin trapped or violated the ABI.
    #[error("plugin trapped: {0}")]
    Trap(String),
    /// The plugin was disabled earlier in this session.
    #[error("plugin is disabled: {0}")]
    Disabled(String),
    /// The event JSON exceeds [`PluginLimits::max_input_bytes`].
    #[error("event input is {size} bytes, limit is {max}")]
    InputTooLarge {
        /// Actual size.
        size: usize,
        /// Limit.
        max: usize,
    },
}

impl PluginError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Identifier of a loaded plugin (the manifest `id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginId(String);

impl PluginId {
    /// The id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for PluginId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// What kind of media was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    /// Instant-replay clip.
    Clip,
    /// Manual recording.
    Recording,
    /// Screenshot.
    Screenshot,
}

/// Event delivered to `hc_on_clip_saved`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipEvent {
    /// Saved file path (sent lossily as a UTF-8 string).
    pub path: PathBuf,
    /// Detected game, if any.
    pub game: Option<String>,
    /// Duration in seconds (0 for screenshots).
    pub duration_s: f64,
    /// Media kind.
    pub kind: ClipKind,
    /// Marker timestamps in seconds from the start of the media.
    pub markers: Vec<f64>,
}

#[derive(Serialize)]
struct ClipEventWire<'a> {
    path: String,
    game: Option<&'a str>,
    duration_s: f64,
    kind: ClipKind,
    markers: &'a [f64],
}

impl ClipEvent {
    /// The exact JSON passed to plugins.
    pub fn to_json(&self) -> String {
        let wire = ClipEventWire {
            path: self.path.to_string_lossy().into_owned(),
            game: self.game.as_deref(),
            duration_s: if self.duration_s.is_finite() {
                self.duration_s
            } else {
                0.0
            },
            kind: self.kind,
            markers: &self.markers,
        };
        // Serializing plain strings/numbers cannot fail; non-finite floats
        // in markers become `null`.
        serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_owned())
    }
}

/// Everything a plugin emitted during one call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginOutput {
    /// Tags to attach to the clip (requires `"tags"`).
    pub tags: Vec<String>,
    /// Notification messages (requires `"notify"`).
    pub notifications: Vec<String>,
    /// Log lines (already forwarded to `tracing`).
    pub logs: Vec<String>,
}

/// Whether a loaded plugin is still being called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// Active.
    Enabled,
    /// Disabled for this session after a failure.
    Disabled {
        /// Human readable reason.
        reason: String,
    },
}

/// A plugin known to the host.
pub struct LoadedPlugin {
    /// Parsed manifest.
    pub manifest: PluginManifest,
    /// Plugin folder.
    pub dir: PathBuf,
    /// Enabled or disabled-with-reason.
    pub status: PluginStatus,
    runtime: Option<Runtime>,
}

impl fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("manifest", &self.manifest)
            .field("dir", &self.dir)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl LoadedPlugin {
    /// The plugin's id.
    pub fn id(&self) -> PluginId {
        PluginId(self.manifest.id.clone())
    }

    /// `true` if the plugin is enabled.
    pub fn is_enabled(&self) -> bool {
        self.status == PluginStatus::Enabled
    }
}

struct Runtime {
    store: Store<HostState>,
    instance: Instance,
    alloc: TypedFunc<i32, i32>,
    on_clip_saved: Option<TypedFunc<(i32, i32), ()>>,
}

struct HostState {
    store_limits: StoreLimits,
    plugin_id: String,
    allow_tags: bool,
    allow_notify: bool,
    limits: PluginLimits,
    output: PluginOutput,
    /// The plugin's exported memory, set right after instantiation.
    ///
    /// Host functions use this handle instead of `Caller::get_export`, which
    /// crashes (access violation) in wasmi 2.0.2 when called from a host
    /// function on this platform.
    memory: Option<Memory>,
}

/// Owns the interpreter engine and all loaded plugins.
pub struct PluginHost {
    engine: Engine,
    linker: Linker<HostState>,
    limits: PluginLimits,
    plugins: Vec<LoadedPlugin>,
}

impl fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginHost")
            .field("limits", &self.limits)
            .field("plugins", &self.plugins)
            .finish_non_exhaustive()
    }
}

impl PluginHost {
    /// Creates a host with fuel metering and the given limits.
    pub fn new(limits: PluginLimits) -> Self {
        let mut config = Config::default();
        config
            .consume_fuel(true)
            .compilation_mode(CompilationMode::Eager)
            .wasm_multi_memory(false);
        let engine = Engine::new(&config);
        let mut linker = Linker::new(&engine);
        define_host_funcs(&mut linker);
        Self {
            engine,
            linker,
            limits,
            plugins: Vec::new(),
        }
    }

    /// The limits applied to plugins.
    pub fn limits(&self) -> &PluginLimits {
        &self.limits
    }

    /// All loaded plugins (enabled and disabled), in load order.
    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    /// Loads the plugin in `dir` (a folder with `plugin.toml` and `plugin.wasm`).
    ///
    /// Validates the manifest, the wasm size, the imports (only `hc.*` with the
    /// right signatures) and the required exports, then instantiates the
    /// module (running its `start` function, if any, under the fuel limit).
    pub fn load(&mut self, dir: &Path) -> Result<PluginId, PluginError> {
        let manifest = PluginManifest::from_dir(dir)?;
        if self.plugins.iter().any(|p| p.manifest.id == manifest.id) {
            return Err(PluginError::AlreadyLoaded(manifest.id));
        }
        let wasm_path = dir.join(WASM_FILE);
        let size = fs::metadata(&wasm_path)
            .map_err(|e| PluginError::io(&wasm_path, e))?
            .len();
        if size > self.limits.max_wasm_bytes {
            return Err(PluginError::WasmTooLarge {
                size,
                max: self.limits.max_wasm_bytes,
            });
        }
        let wasm = fs::read(&wasm_path).map_err(|e| PluginError::io(&wasm_path, e))?;
        self.load_bytes(manifest, dir.to_path_buf(), &wasm)
    }

    /// Loads a plugin from an in-memory manifest and module (`dir` is only
    /// informational). Same checks as [`PluginHost::load`].
    pub fn load_bytes(
        &mut self,
        manifest: PluginManifest,
        dir: PathBuf,
        wasm: &[u8],
    ) -> Result<PluginId, PluginError> {
        if self.plugins.iter().any(|p| p.manifest.id == manifest.id) {
            return Err(PluginError::AlreadyLoaded(manifest.id));
        }
        let size = wasm.len() as u64;
        if size > self.limits.max_wasm_bytes {
            return Err(PluginError::WasmTooLarge {
                size,
                max: self.limits.max_wasm_bytes,
            });
        }
        let runtime = self.instantiate(&manifest, wasm)?;
        let id = PluginId(manifest.id.clone());
        tracing::info!(plugin = %id, version = %manifest.version, "plugin loaded");
        self.plugins.push(LoadedPlugin {
            manifest,
            dir,
            status: PluginStatus::Enabled,
            runtime: Some(runtime),
        });
        Ok(id)
    }

    /// Discovers and loads every plugin under `plugins_dir`.
    ///
    /// Returns one result per discovered folder.
    pub fn load_all(
        &mut self,
        plugins_dir: &Path,
    ) -> Vec<Result<PluginId, (PathBuf, PluginError)>> {
        discover(plugins_dir)
            .into_iter()
            .map(|entry| {
                let manifest = entry?;
                let dir = plugins_dir.join(&manifest.id);
                self.load(&dir).map_err(|e| (dir, e))
            })
            .collect()
    }

    /// Unloads a plugin, dropping its instance and memory.
    pub fn unload(&mut self, id: &PluginId) -> Result<(), PluginError> {
        let idx = self
            .plugins
            .iter()
            .position(|p| p.manifest.id == id.0)
            .ok_or_else(|| PluginError::NotFound(id.0.clone()))?;
        self.plugins.remove(idx);
        tracing::info!(plugin = %id, "plugin unloaded");
        Ok(())
    }

    /// Delivers a saved-clip event to every enabled plugin that exports
    /// `hc_on_clip_saved`.
    ///
    /// Plugins without the handler are skipped. Disabled plugins are skipped
    /// too (they were already reported when they failed). A plugin that traps
    /// or runs out of fuel is disabled and reported with its error.
    pub fn on_clip_saved(
        &mut self,
        event: &ClipEvent,
    ) -> Vec<(PluginId, Result<PluginOutput, PluginError>)> {
        let input = event.to_json();
        let mut results = Vec::new();
        for plugin in &mut self.plugins {
            if !plugin.is_enabled() {
                continue;
            }
            let Some(runtime) = plugin.runtime.as_mut() else {
                continue;
            };
            let Some(handler) = runtime.on_clip_saved else {
                continue;
            };
            let id = PluginId(plugin.manifest.id.clone());
            let result = call_handler(runtime, &self.limits, &input, handler);
            if let Err(err) = &result
                && !matches!(err, PluginError::InputTooLarge { .. })
            {
                tracing::warn!(plugin = %id, error = %err, "plugin disabled");
                plugin.status = PluginStatus::Disabled {
                    reason: err.to_string(),
                };
                // The instance may be in an inconsistent state; drop it.
                plugin.runtime = None;
            }
            results.push((id, result));
        }
        results
    }

    fn instantiate(&self, manifest: &PluginManifest, wasm: &[u8]) -> Result<Runtime, PluginError> {
        let module = Module::new(&self.engine, wasm)
            .map_err(|e| PluginError::InvalidModule(e.to_string()))?;
        check_imports(&module)?;
        check_exports(&module)?;

        let state = HostState {
            store_limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .table_elements(65_536)
                .instances(1)
                .memories(1)
                .tables(1)
                .build(),
            plugin_id: manifest.id.clone(),
            allow_tags: manifest.has_permission(Permission::Tags),
            allow_notify: manifest.has_permission(Permission::Notify),
            limits: self.limits.clone(),
            output: PluginOutput::default(),
            memory: None,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.store_limits);
        set_fuel(&mut store, self.limits.fuel_per_call)?;
        let instance = self
            .linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| match classify(&e, self.limits.fuel_per_call) {
                PluginError::Trap(msg) => PluginError::Instantiation(msg),
                other => other,
            })?;
        // Output produced by a start function is discarded.
        store.data_mut().output = PluginOutput::default();
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| bad_export("memory", "missing"))?;
        store.data_mut().memory = Some(memory);

        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "hc_alloc")
            .map_err(|e| bad_export("hc_alloc", e))?;
        let on_clip_saved = match instance.get_func(&store, "hc_on_clip_saved") {
            None => None,
            Some(f) => Some(
                f.typed::<(i32, i32), ()>(&store)
                    .map_err(|e| bad_export("hc_on_clip_saved", e))?,
            ),
        };
        Ok(Runtime {
            store,
            instance,
            alloc,
            on_clip_saved,
        })
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new(PluginLimits::default())
    }
}

fn bad_export(name: &str, err: impl fmt::Display) -> PluginError {
    PluginError::BadExport {
        name: name.to_owned(),
        reason: err.to_string(),
    }
}

fn set_fuel(store: &mut Store<HostState>, fuel: u64) -> Result<(), PluginError> {
    store
        .set_fuel(fuel)
        .map_err(|e| PluginError::Trap(format!("cannot set fuel: {e}")))
}

/// Maps a wasmi runtime error to a [`PluginError`].
fn classify(err: &wasmi::Error, fuel: u64) -> PluginError {
    let out_of_fuel = err.as_trap_code() == Some(TrapCode::OutOfFuel)
        || matches!(err.kind(), ErrorKind::Fuel(FuelError::OutOfFuel { .. }));
    if out_of_fuel {
        PluginError::OutOfFuel(fuel)
    } else {
        PluginError::Trap(err.to_string())
    }
}

fn check_imports(module: &Module) -> Result<(), PluginError> {
    for import in module.imports() {
        let forbidden = || PluginError::ForbiddenImport {
            module: import.module().to_owned(),
            name: import.name().to_owned(),
        };
        if import.module() != HOST_MODULE || !HOST_FUNCS.contains(&import.name()) {
            return Err(forbidden());
        }
        let ExternType::Func(ty) = import.ty() else {
            return Err(forbidden());
        };
        if ty.params() != [ValType::I32, ValType::I32] || ty.results() != [ValType::I32] {
            return Err(PluginError::InvalidModule(format!(
                "import hc.{} must have type (i32, i32) -> i32",
                import.name()
            )));
        }
    }
    Ok(())
}

fn check_exports(module: &Module) -> Result<(), PluginError> {
    match module.get_export("memory") {
        Some(ExternType::Memory(_)) => {}
        Some(_) => return Err(bad_export("memory", "must be a memory")),
        None => return Err(bad_export("memory", "missing")),
    }
    match module.get_export("hc_alloc") {
        Some(ExternType::Func(_)) => {}
        Some(_) => return Err(bad_export("hc_alloc", "must be a function")),
        None => return Err(bad_export("hc_alloc", "missing")),
    }
    Ok(())
}

fn call_handler(
    rt: &mut Runtime,
    limits: &PluginLimits,
    input: &str,
    handler: TypedFunc<(i32, i32), ()>,
) -> Result<PluginOutput, PluginError> {
    let bytes = input.as_bytes();
    if bytes.len() > limits.max_input_bytes {
        return Err(PluginError::InputTooLarge {
            size: bytes.len(),
            max: limits.max_input_bytes,
        });
    }
    let len = i32::try_from(bytes.len()).map_err(|_| PluginError::InputTooLarge {
        size: bytes.len(),
        max: limits.max_input_bytes,
    })?;
    rt.store.data_mut().output = PluginOutput::default();
    set_fuel(&mut rt.store, limits.fuel_per_call)?;
    let ptr = rt
        .alloc
        .call(&mut rt.store, len)
        .map_err(|e| classify(&e, limits.fuel_per_call))?;
    let memory = rt
        .instance
        .get_memory(&rt.store, "memory")
        .ok_or_else(|| bad_export("memory", "missing"))?;
    let offset = usize::try_from(ptr)
        .map_err(|_| PluginError::Trap(format!("hc_alloc returned invalid pointer {ptr}")))?;
    memory.write(&mut rt.store, offset, bytes).map_err(|_| {
        PluginError::Trap(format!(
            "hc_alloc returned out-of-bounds buffer {ptr}+{len}"
        ))
    })?;
    handler
        .call(&mut rt.store, (ptr, len))
        .map_err(|e| classify(&e, limits.fuel_per_call))?;
    Ok(std::mem::take(&mut rt.store.data_mut().output))
}

/// Bounds-checks `ptr..ptr+len` against the caller's memory and copies at most
/// `max_copy` bytes from its start. Traps (returns `Err`) when out of bounds.
fn read_guest(
    caller: &Caller<'_, HostState>,
    ptr: i32,
    len: i32,
    max_copy: usize,
) -> Result<Vec<u8>, wasmi::Error> {
    let (Ok(offset), Ok(len)) = (usize::try_from(ptr), usize::try_from(len)) else {
        return Err(wasmi::Error::new("negative pointer or length"));
    };
    let Some(memory) = caller.data().memory else {
        // Only possible from a `start` function: host calls are unavailable there.
        return Err(wasmi::Error::new(
            "host functions cannot be called from a start function",
        ));
    };
    let data = memory.data(caller);
    let end = offset
        .checked_add(len)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| wasmi::Error::new("pointer/length out of bounds"))?;
    let copy_end = end.min(offset.saturating_add(max_copy));
    Ok(data[offset..copy_end].to_vec())
}

fn truncate_utf8(s: &mut String, max: usize) {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
}

fn host_log(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> Result<i32, wasmi::Error> {
    let max = caller.data().limits.max_log_bytes;
    // Copy a few extra bytes so a multi-byte char at the cut is not mangled.
    let bytes = read_guest(&caller, ptr, len, max.saturating_add(3))?;
    let state = caller.data_mut();
    if state.output.logs.len() >= state.limits.max_logs_per_call {
        return Ok(RC_LIMIT);
    }
    let mut line = String::from_utf8_lossy(&bytes).into_owned();
    truncate_utf8(&mut line, max);
    tracing::info!(plugin = %state.plugin_id, "{line}");
    state.output.logs.push(line);
    Ok(RC_OK)
}

fn host_add_tag(
    mut caller: Caller<'_, HostState>,
    ptr: i32,
    len: i32,
) -> Result<i32, wasmi::Error> {
    let max = caller.data().limits.max_tag_bytes;
    let bytes = read_guest(&caller, ptr, len, max.saturating_add(1))?;
    let state = caller.data_mut();
    if !state.allow_tags {
        return Ok(RC_DENIED);
    }
    if bytes.len() > max || state.output.tags.len() >= state.limits.max_tags_per_call {
        return Ok(RC_LIMIT);
    }
    let Ok(tag) = std::str::from_utf8(&bytes) else {
        return Ok(RC_INVALID_UTF8);
    };
    let tag = tag.trim();
    if tag.is_empty() {
        return Ok(RC_LIMIT);
    }
    if !state.output.tags.iter().any(|t| t == tag) {
        state.output.tags.push(tag.to_owned());
    }
    Ok(RC_OK)
}

fn host_notify(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> Result<i32, wasmi::Error> {
    let max = caller.data().limits.max_notification_bytes;
    let bytes = read_guest(&caller, ptr, len, max.saturating_add(1))?;
    let state = caller.data_mut();
    if !state.allow_notify {
        return Ok(RC_DENIED);
    }
    if bytes.len() > max
        || state.output.notifications.len() >= state.limits.max_notifications_per_call
    {
        return Ok(RC_LIMIT);
    }
    let Ok(msg) = std::str::from_utf8(&bytes) else {
        return Ok(RC_INVALID_UTF8);
    };
    let msg = msg.trim();
    if msg.is_empty() {
        return Ok(RC_LIMIT);
    }
    state.output.notifications.push(msg.to_owned());
    Ok(RC_OK)
}

fn define_host_funcs(linker: &mut Linker<HostState>) {
    // Defining three distinct names on a fresh linker cannot collide.
    for result in [
        linker.func_wrap(HOST_MODULE, "log", host_log).map(|_| ()),
        linker
            .func_wrap(HOST_MODULE, "add_tag", host_add_tag)
            .map(|_| ()),
        linker
            .func_wrap(HOST_MODULE, "notify", host_notify)
            .map(|_| ()),
    ] {
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to define plugin host function");
        }
    }
}

#[cfg(test)]
mod tests;
