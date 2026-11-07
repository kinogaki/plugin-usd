# USD

Universal Scene Description as a format plugin for [Kinogaki Studio](https://github.com/kinogaki): usda, usdc and usdz files read and written, stages composed, and the scene they describe read into plain data and authored back. Rust with no dependencies beyond the public [studio-sdk](https://github.com/kinogaki/studio-sdk) (for its workers) and no OpenUSD code.

The library crate is `kinogaki-usd`. Package `kinogaki.usd`, format `kinogaki.format.usd` (`usd`, `usda`, `usdc`, `usdz`), with `plugin/`, the dylib Studio loads (`formats/usd.dylib`), once the format is wired to the SDK's format contract. Until then this repository holds the format's own model, parser and writer, `src/usd/`, which knows nothing of the SDK but its workers.

## Reading

- **Layers** (`usd::sdf`, `usd::text`, `usd::crate_file`, `usd::package`): usda text; usdc crates of versions 0.0 to 0.15; usdz packages (stored, 64-byte aligned, nested), every entry read in place. A layer is recognized by its signature whatever its extension says.
- **Composition** (`usd::compose`, after Pcp): sublayers with offsets and rates, references, payloads (with load rules), variants (authored, session and fallback selections), inherits, specializes, relocates, variable expressions, and instancing. A stage opens from a path, or from bytes in memory (an embedded file), which names no file beside it. A reference to a MaterialX document (`@look.mtlx@`) is kept for MaterialX's reader instead of composed.
- **The scene** (`usd::scene`): a tree of prims, parents first, each with its local transform, `!resetXformStack!`, visibility and bound material, holding
  - a Mesh with its primvars (the layer's arrays, shared, with interpolation, element size, indices and color space), subdivision tags, GeomSubsets and their bindings;
  - an implicit shape (Cube, Sphere, Cylinder, Cone, Capsule, Plane) as authored, tessellated on request;
  - BasisCurves (linear, Bézier, B-spline, Catmull-Rom; nonperiodic, periodic, pinned) and NurbsCurves; Points;
  - a UsdGeomCamera; a UsdLux light (Rect, Disk, Dome with its image, Distant, Sphere ± ShapingAPI's cone);
  - a PointInstancer placing prototypes; an instanceable prim showing a shared prototype.

  Values are read at one time code. Purposes `proxy` and `guide` load when asked.
- **Materials** (`usd::shade`): every Material prim as its UsdShade network as authored: its terminal outputs per render context (`outputs:surface` USD Preview Surface, `outputs:mtlx:surface` MaterialX, `outputs:kinogaki:surface` an engine's own nodes), its interface inputs, and its Shader and NodeGraph prims with their `info:*`, inputs, outputs, color spaces and connections. Bindings follow UsdShade: full purpose before all-purpose, `strongerThanDescendants`, inherited down the tree.
- **Assets**: the files materials and lights name, a package's entries held as its bytes (no copy), files on disk by path.

## Writing

A scene authors as one layer (`defaultPrim`, `upAxis`, `metersPerUnit`; prims, bindings, materials) and writes as usda, usdc 0.8, or a usdz carrying its assets under `textures/`; a loose layer writes the assets it holds beside it, in `<stem>_textures/`. Layers write as `usdcat` prints and writes them.

## Code

| Path | What |
|---|---|
| `src/usd/sdf/` | The data model: tokens, paths, values, list ops, layers of specs |
| `src/usd/text/`, `src/usd/crate_file/`, `src/usd/package/` | usda, usdc, usdz |
| `src/usd/compose/` | Stages: layer stacks, prim indexes, flattening |
| `src/usd/scene/` | The scene model, its reading (`read.rs`) and authoring (`write/`) |
| `src/usd/shade/` | UsdShade networks, read and authored |
| `examples/kusd.rs` | Prints, converts, flattens and times layers; drives the parity checks |
| `tools/parity.py` | Differential checks against OpenUSD's `usdcat` (local) |

## Tests and data

`cargo test` runs the unit tests and `tests/`: composition (`compose.rs`), the scene (`scene.rs`), materials (`shade.rs`), instancing (`instancing.rs`), packages and memory input (`package.rs`), and mutation fuzzing of text, crates and packages (`fuzz.rs`). Every fixture under `tests/fixtures/` is written for this repository and is under its licence. Larger corpora are local only: OpenUSD's own test layers through `tools/parity.py`, and Pixar's Kitchen Set (not redistributable) for timing.

## Building

The format builds from this repository and the public studio-sdk, checked out side by side as Studio's tree lays them out (`sdk/` and `plugins/usd/`), with the compiler the SDK pins (`sdk/rust-toolchain.toml`):

```sh
git clone https://github.com/kinogaki/studio-sdk sdk
git clone https://github.com/kinogaki/plugin-usd plugins/usd
cd plugins/usd
cargo test
```

## License

Dual-licensed under the MIT license ([LICENSE-MIT](LICENSE-MIT)) and the Apache License 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), at your option. The files derived from OpenUSD keep OpenUSD's licence, the Tomorrow Open Source Technology License 1.0 ([licenses/OPENUSD-TOST-1.0.txt](licenses/OPENUSD-TOST-1.0.txt)), and Pixar's notice: see [NOTICE](NOTICE).
