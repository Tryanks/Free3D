# Contributing to Free3D

Thanks for your interest in Free3D! Whether you're reporting a bug, suggesting a feature, improving translations, or writing code, contributions of every kind are welcome — and you don't need to be a CAD expert to help.

## Reporting bugs

Open a [GitHub issue](../../issues) and include:

- What you did, what you expected, and what happened instead
- Your OS and version (e.g. macOS 15.5, Windows 11, Ubuntu 24.04)
- A screenshot or screen recording if the problem is visual
- The `.f3d` file if the problem is specific to a design (only if you're comfortable sharing it)

Crashes: run the app from a terminal and paste the panic message printed there. That single message usually cuts the diagnosis time in half.

## Suggesting features

Open an issue describing **what you're trying to accomplish**, not only the feature you have in mind — knowing the goal often reveals a better design. Screenshots or links showing how other CAD tools handle it are very helpful.

## Building from source

Prerequisites: [Rust](https://rustup.rs) 1.95+ and CMake 3.24+. On Linux, gpui additionally needs common desktop development headers (see the apt list in `.github/workflows/release.yml`).

```sh
git clone https://github.com/Tryanks/Free3D.git
cd Free3D
cargo run
```

The first build compiles OpenCASCADE from source (~15 minutes). `.cargo/config.toml` already sets `CMAKE_POLICY_VERSION_MINIMUM=3.5` for CMake 4.x compatibility.

## Development tips

Environment variables that make development and testing easier:

| Variable | Effect |
|---|---|
| `FREE3D_DEMO_SCENE=1..9` | Boot straight into a prepared scene (1 solids, 4 sketch, 6 drawing with BOM, 9 assembly…) |
| `FREE3D_DUMP_FRAME=/path.png` | Write every rendered viewport frame to disk — headless render verification |
| `FREE3D_IO_CHECK=1` | Run a headless STEP/OBJ/glTF/… round-trip check and exit |
| `FREE3D_LANG=en\|zh-CN` | Force the UI language |
| `FREE3D_SETTINGS_DIR`, `FREE3D_DESIGNS_DIR` | Redirect settings / design library (isolated test profiles) |

Project layout: `src/` is the application (viewport, tools, UI, document/history); `crates/occt-bridge` is the cxx bridge to OpenCASCADE (every throwing call returns `Result`); `crates/occt` is the safe Rust geometry API on top of it.

## Pull requests

Before opening a PR, please make sure:

```sh
cargo fmt --check   # formatting
cargo test --workspace   # full test suite — keep it green
cargo build 2>&1 | grep warning   # zero warnings expected
```

- Keep PRs focused — one topic per PR is much easier to review.
- New user-visible strings go through `crate::i18n::t(...)` with a Simplified Chinese entry in `src/i18n.rs`.
- New behavior needs a test when the logic is testable headlessly (geometry, document ops, formatters).
- GPU/rendering changes deserve a manual check with `FREE3D_DUMP_FRAME` — WGSL errors only surface on a real GPU at runtime.

Small fixes are welcome without prior discussion; for larger changes, opening an issue first saves everyone time.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind.
