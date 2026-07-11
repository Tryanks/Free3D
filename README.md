# Free3D

An open-source direct-modeling CAD application inspired by Shapr3D's interaction design, written in Rust:

- **UI framework**: [gpui](https://github.com/zed-industries/zed) (the GPU-accelerated UI framework behind the Zed editor, used as a git dependency)
- **Geometry kernel**: OpenCASCADE (OCCT 7.8.1) through an in-house binding layer (`crates/occt-bridge` cxx bridge + `crates/occt` safe API, statically compiled via occt-sys); every OCCT exception is converted into a `Result`
- **Viewport rendering**: wgpu offscreen rendering (BGRA readback → gpui `RenderImage`), MSAA 4x, redraws only during interaction

All icons and artwork are original; this project emulates interaction patterns, not any copyrighted assets. The UI copy is currently Simplified Chinese.

## Build & Run

```sh
# Requirements: Rust ≥1.95, CMake (brew install cmake), Xcode CLT
cargo run
```

The first build compiles OCCT from source (about 15 minutes); incremental builds are fast afterwards.
`.cargo/config.toml` ships with `CMAKE_POLICY_VERSION_MINIMUM=3.5` (required for CMake 4.x to accept OCCT's older build scripts).

## Interaction (default preset, touchpad-first)

| Action | Touchpad | Mouse |
|---|---|---|
| Orbit | Two-finger drag | Right-button drag |
| Pan | Shift + two-finger drag | Middle-button drag |
| Zoom | Pinch (toward cursor) | Scroll wheel (toward cursor) |

⌘1 default isometric view, ⌘2–7 the six standard views (animated transitions); the **orientation cube** in the top-right corner snaps the camera to faces/edges/corners, rotates on drag, and resets on double-click. ⌘Z / ⇧⌘Z undo/redo. Tab cycles the selection filter (body/face/edge); Esc clears the selection. Navigation presets and light/dark themes are switchable in Settings.

## Feature Overview

- **Direct modeling**: select a face and drag to push/pull (pull = union / push = subtract with automatic booleans; a badge switches New Body/Union/Subtract/Intersect); move/rotate gizmo (live preview, Shift 5° snapping); fillet/chamfer by dragging edges; shell; offset face; explicit booleans
- **Sketching & features**: full 2D geometry with trim/extend/break and a constraint solver; closed profiles extrude/revolve/sweep/loft; holes, threads, variable-radius fillets and other history features can reference named variables
- **Surfaces**: open-profile surface extrude/revolve/sweep/loft, plus patch, sew, thicken, and delete-face healing
- **Transforms**: move/rotate/scale/mirror/linear & circular patterns/align/split body
- **Parametrics & recovery**: history steps can be edited, suppressed, deleted, and replayed to rebuild; feature parameters accept named variables; native `.f3d` format, autosave, and crash recovery
- **Inspection & views**: volume/centroid/moments of inertia, interference and geometric validity checks, X-Ray/section interference highlighting; isolate, measure, saved views, FOV, and an adaptive grid
- **Assemblies**: same-document component grounding, five joint kinds, numeric driving, continuous exploded view
- **Drawings**: multi-sheet HLR views, section/detail views, linear/radius/diameter/angle dimensions, centerlines, title block, live BOM with associative balloons, SVG/PDF export
- **Visualization**: per-entity PBR-lite materials and colors
- **Selection**: click/double-click for bodies, Shift multi-select, box select (left→right window, right→left crossing), hover highlighting — all BRep-based picking
- **Adaptive menu**: tool recommendations follow the current selection (Shapr3D's signature interaction)
- **Files**: STEP/IGES/STL/DXF and other common CAD/mesh exchange formats, OBJ/3MF/glTF export; command search palette
- **Type a number while dragging** for exact values (expressions like 12+34 and 50/2 are supported)

## Developer Notes

- `FREE3D_DUMP_FRAME=/path.png` writes every viewport frame to disk (headless render verification without screen-recording permissions)
- `FREE3D_DEMO_SCENE=1..9` preset demo scenes; `6` shows a drawing with BOM/balloons, `9` shows an assembly
- `FREE3D_IO_CHECK=1` headless STEP round-trip check

## License

[MIT](LICENSE). Note that OCCT is LGPL-2.1 (with exception); distributing statically linked builds must comply with its terms.
