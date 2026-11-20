# hc-plugins

Sandboxed WebAssembly plugin host for Hyper Clips, built on the pure-Rust
[wasmi](https://crates.io/crates/wasmi) interpreter (no JIT, no WASI).

## Plugin layout

```text
<plugins dir>/<id>/plugin.toml
<plugins dir>/<id>/plugin.wasm     # max 4 MiB by default
```

```toml
id = "game-tagger"            # [a-z0-9_.-]{1,64}, must equal the folder name
name = "Game Tagger"
version = "1.0.0"
description = "Tags every clip with the detected game"
author = "you"
api = 1
permissions = ["tags", "notify"]   # the only capabilities that exist
```

## Sandbox

- Only imports from module `hc` (`log`, `add_tag`, `notify`) are allowed; any
  other import (WASI, `env`, ...) is rejected at load time. Plugins therefore
  have no filesystem, network, clock or randomness.
- Fuel metering: 50M fuel by default for instantiation (including a `start`
  function) and again per event (`hc_alloc` + handler share it). Out of fuel
  = error.
- Linear memory capped at 32 MiB by default; `memory.grow` past it returns -1.
- Output limits per call: 32 log lines (1 KiB each, truncated), 16 tags
  (64 bytes each), 4 notifications (512 bytes each).
- Any trap, out-of-fuel or ABI violation disables the plugin for the session.
  The host never panics because of a plugin.
- Not guaranteed: a plugin can burn CPU up to the fuel limit per event and
  use memory up to the cap. Calls run synchronously on the caller's thread.

## ABI v1

Plugin exports:

| export | required | |
|---|---|---|
| `memory` | yes | linear memory |
| `hc_alloc(len: i32) -> i32` | yes | returns a buffer of `len` bytes; the host writes the event JSON there |
| `hc_on_clip_saved(ptr: i32, len: i32)` | no | event handler |

Event JSON:

```json
{"path":"C:\\clips\\a.mp4","game":"Valorant","duration_s":30.0,"kind":"clip","markers":[1.5,10.0]}
```

`kind` is `clip`, `recording` or `screenshot`; `game` may be `null`;
`markers` are seconds from the start of the media.

Host imports (module `hc`, all `(ptr: i32, len: i32) -> i32`, UTF-8 strings):

| import | permission | |
|---|---|---|
| `log` | none | log line |
| `add_tag` | `tags` | tag for the saved clip |
| `notify` | `notify` | user notification |

Return codes: `0` ok, `-1` permission denied (ignored, plugin keeps
running), `-2` size/count limit exceeded (dropped), `-3` invalid UTF-8.
An out-of-bounds or negative `ptr`/`len` traps and disables the plugin.
Host functions cannot be called from a wasm `start` function.

## Minimal Rust plugin

`Cargo.toml`:

```toml
[package]
name = "game-tagger"
version = "1.0.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
```

`src/lib.rs`:

```rust
use serde::Deserialize;

#[link(wasm_import_module = "hc")]
unsafe extern "C" {
    fn log(ptr: *const u8, len: usize) -> i32;
    fn add_tag(ptr: *const u8, len: usize) -> i32;
}

fn hc_log(msg: &str) {
    // SAFETY: the pointer/length describe a valid UTF-8 buffer in our memory.
    unsafe { log(msg.as_ptr(), msg.len()) };
}

#[derive(Deserialize)]
struct ClipEvent {
    game: Option<String>,
    kind: String,
}

/// The host writes the event JSON into this buffer. It is leaked on purpose
/// and reclaimed in `hc_on_clip_saved`.
#[unsafe(no_mangle)]
pub extern "C" fn hc_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[unsafe(no_mangle)]
pub extern "C" fn hc_on_clip_saved(ptr: *mut u8, len: usize) {
    // SAFETY: `ptr` came from `hc_alloc(len)` and the host wrote `len` bytes.
    let bytes = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let Ok(event) = serde_json::from_slice::<ClipEvent>(&bytes) else {
        hc_log("could not parse event");
        return;
    };
    if let Some(game) = event.game {
        // SAFETY: as above.
        if unsafe { add_tag(game.as_ptr(), game.len()) } == -1 {
            hc_log("missing \"tags\" permission");
        }
    }
    hc_log(&format!("tagged a {}", event.kind));
}
```

Build (no WASI target — `wasm32-unknown-unknown` has no system imports):

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/game_tagger.wasm <plugins dir>/game-tagger/plugin.wasm
```

`usize` and pointers are 32-bit on `wasm32`, matching the `i32` ABI. Avoid
crates that pull in `wasm-bindgen` or WASI imports; the host will reject a
module that imports anything outside `hc`.

## Host API

```rust
let mut host = PluginHost::new(PluginLimits::default());
for result in host.load_all(&plugins_dir) { /* report errors */ }
for (id, result) in host.on_clip_saved(&event) {
    // Ok(PluginOutput { tags, notifications, logs }) or Err(PluginError)
}
```
