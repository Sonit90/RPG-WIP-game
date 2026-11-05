# macOS (Apple Silicon)

The game runs on Apple Silicon Macs (macOS 13+) with a Metal renderer. Intel Macs are not
supported (target `aarch64-apple-darwin`).

## Prerequisites

* Xcode (the full app, not only the command line tools) - the Metal shader compiler ships with
  it. With Xcode 26 the Metal toolchain is a separate download:

  ```sh
  xcodebuild -downloadComponent MetalToolchain
  ```

* Rust stable via `rustup`. On an Apple Silicon Mac the default target is already right:

  ```sh
  rustup target add aarch64-apple-darwin   # no-op if already installed
  ```

## Build and run

```sh
cargo run -p engine-app --release
```

The command-line options are the same as on Windows (`cargo run -p engine-app -- --help`).

## Shaders

`crates/engine-renderer/build.rs` compiles `shaders/metal/*.metal` with `xcrun` (`metal`, then
`metallib`) into a `.metallib` embedded in the binary. If the Metal toolchain is unavailable the
build prints a cargo warning and the shaders are compiled from source at runtime instead
(slower startup). Set `MYGAME_METAL_RUNTIME_COMPILE=1` to force the runtime path, for example
when iterating on a shader.

## Where files live

| What | Location |
|---|---|
| `settings.cfg`, `hud.cfg`, `character.cfg` | `~/Library/Application Support/mygame/` when run from an app bundle; beside the executable otherwise (`cargo run`, same as Windows) |
| Remembered login session | `~/Library/Application Support/mygame/session[-profile].bin` (holds a Keychain reference, not the token) |
| Standalone build, unpacked content | `~/Library/Caches/mygame/assets-<hash>/` |
| Content in a bundle | `MyGame.app/Contents/Resources/assets` (searched right after the executable's own `assets/`) |

A bundle is never written into: a signed bundle would break.

## Packaging

```sh
tools/package-macos.sh              # -> dist/macos/MyGame.app
tools/package-macos.sh --no-overlay
```

It builds `--bin mygame --no-default-features` (no `dev-tools`), assembles the bundle
(`Info.plist`, `Contents/MacOS/mygame`, `Contents/Resources/assets`), verifies that the binary
has no developer tools or test/benchmark symbols, ad-hoc signs it (`codesign -s -`) and runs a
two-frame smoke test. Start it with `open dist/macos/MyGame.app`. An ad-hoc signature is not
notarised: on another Mac, clear quarantine with `xattr -dr com.apple.quarantine MyGame.app`.

## Type-checking the macOS target from Windows

`tools/macos-check/check.sh` runs cargo for `aarch64-apple-darwin` without a Mac, using zig as a
stand-in C compiler for build scripts (needs `rustup target add aarch64-apple-darwin` and
`python -m pip install ziglang`). Nothing is linked and no SDK is needed:

```sh
tools/macos-check/check.sh check -p engine-app
tools/macos-check/check.sh clippy -p engine-settings --all-targets -- -D warnings
```

This proves the macOS code compiles; it does not replace running on a Mac.

## Known differences from Windows

* Fullscreen is borderless fullscreen in its own Space (the green window button).
* Cmd acts as Ctrl for shortcuts.
* Secrets: one Keychain item per sealed secret; the file on disk is only a reference to it.
* GPU per-pass timings and pipeline statistics are unavailable on Metal; the overlay omits them.
* The Metal renderer is a separate backend (`crates/engine-renderer/src/metal/`) with the same
  public `Renderer` API as D3D12; shared CPU code (culling, render lists, light clusters,
  `ShadowPlan`, UI batching, atlases, mips) is used by both. Contact shadows sample the camera
  depth directly instead of copying it.
* Shaders are compiled without fast math so the depth prepass and the camera pass agree
  bit-for-bit (primed draws test `LessEqual` against the prepass).

## First run on a Mac: test checklist

Nothing below has run on Apple hardware yet; the macOS code is type-checked from Windows only.
Work through it in order, with Metal API validation on
(`METAL_DEVICE_WRAPPER_TYPE=1 MTL_DEBUG_LAYER=1 cargo run -p engine-app`).

1. **Build.** `cargo build -p engine-app` compiles `shaders/metal` with `xcrun`; any MSL error
   fails the build with the compiler's message. The likeliest spot is the `SceneResources`
   aggregate in `scene_frame.h` (textures inside a local struct) — if it is rejected, pass its
   members as separate parameters. Also try `MYGAME_METAL_RUNTIME_COMPILE=1`.
2. **Window and input.** Window opens at the right physical size on Retina; resize, minimise,
   restore, fullscreen in/out; mouse-look (locked cursor, raw deltas), wheel/trackpad scroll
   speed, keyboard incl. numpad, Cmd+V paste, Cmd+Q quits.
3. **Basic frame.** Clear colour, UI, fonts, icons, debug overlay (F5), map picture.
4. **Scene.** Lit scene with PBR and Lambert materials, normal maps, local lights; instanced
   vs per-object look identical; no missing faces (if faces vanish, flip the winding in
   `metal/recorder.rs`); wireframe; transparent, dithered/camera-fade and translucent
   characters; skinned characters; particles (additive and alpha); BC1/BC5 textures.
5. **Shadows.** Cascades and local-light shadows look like Windows with default bias (no acne,
   no peter-panning); alpha-tested foliage casts leaf shapes; casters beyond the cascade near
   plane still cast (depth clamp); no holes in primed geometry (if there are, add
   `[[invariant]]` to the clip position — see the note in `metal/shadow.rs`); contact shadows
   on/off; shadow debug modes 1–3; change preset/resolution at runtime.
6. **Screenshots.** Screenshot colours are not swapped (BGRA → RGBA conversion).
7. **Stability.** Run for several minutes: memory and `upload_peak_bytes` stay flat; frames in
   flight 1/2/3; resizing while running; clean shutdown with no Metal assertions.
8. **Online.** Sign in, quit, relaunch — the session is remembered (Keychain; the first access
   may prompt). Two `--profile`s keep separate sessions.
9. **Package.** `tools/package-macos.sh` produces `dist/macos/MyGame.app`, which launches from
   Finder and keeps settings in `~/Library/Application Support/mygame`.
