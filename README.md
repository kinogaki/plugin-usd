# USD

Universal Scene Description as a format plugin for [Kinogaki Studio](https://github.com/kinogaki): usda, usdc and usdz files read into the standard values of the public [studio-sdk](https://github.com/kinogaki/studio-sdk) and written from them, stages composed, materials kept in every render context they are authored in. Rust with no dependencies beyond the SDK and no OpenUSD code.

The library crate is `kinogaki-usd`; `plugin/` (`kinogaki-plugin-usd`) is the dylib Studio loads, `formats/usd.dylib`. Package `kinogaki.usd`, format `kinogaki.format.usd` (`usd`, `usda`, `usdc`, `usdz`). The format's own model, parser and writer is `src/usd/`, which knows nothing of the SDK but its workers and what bounds a read (the read's budget, and the resolver for the files a file names); the format converts between its scene and the standard values at its boundary (`src/read/`, `src/write/`) and nowhere else.

## Import

File ▸ Import (and a File node) reads a file into the project's units: `metersPerUnit` and `upAxis` change the points and every placement's frame, not the placements' scale, so the hierarchy keeps its shape. Items come parents first:

- **Materials** first, one item: every Material prim keyed by its path, named by its `kinogaki:name` (else its prim's), double-sided as its `kinogaki:doubleSided` says, else unless a single-sided mesh shows it (a mesh's `doubleSided` is its material's Double Sided), its lengths (glTF PBR's thickness, a transmission depth) moved from the stage's units into the project's as its points are. It keeps the first network it holds of an engine's own (`outputs:kinogaki:surface`, each shader's `info:id` a node type's id), MaterialX's (`outputs:mtlx:surface`, each `info:id` a MaterialX definition; a shader holding inputs its definition lacks is the Kinogaki definition inheriting it that adds them) and USD Preview Surface (`outputs:surface`, its shaders as MaterialX's `UsdPreviewSurface`, `UsdUVTexture`, `UsdPrimvarReader` and `UsdTransform2d` with USD's inputs, `opacityMode` and wraps translated); one holding an engine's keeps the next as its standard form, which Studio takes where that engine isn't installed. Shaders no terminal reaches travel with the kept network. Node graphs and interface inputs are looked through. A texture's file is an Image node wired into the input that names it: a usdz's entries embedded as ranges of the file read (never copied), files on disk by their absolute path, or read in with **Embed Textures**; a file outside the read file's folder is left unresolved and said unless **Read Files Outside the Folder** is on; its color is `sourceColorSpace` or the input's `colorSpace`. Colors convert from the space a value says (linear Rec.709 when it says none) into the working space. A primvar reader naming `st` names `uv`.
- **Prims**, each placed where the stage places it (`!resetXformStack!` kept), hidden where invisible: groups; meshes and implicit shapes as polygons (faces repeating a point, of fewer than three corners, or holes left out and said; left-handed meshes wound the other way), each primvar an attribute on the domain its interpolation names (`st` is `uv`, `displayColor` `Cd`, `displayOpacity` `Alpha`, `normals` `N`; points, vectors and normals moved into the project; text primvars as text), each face's `material` its GeomSubset's binding over its mesh's, GeomSubsets as groups; curves (linear, Bézier, B-spline, Catmull-Rom, NURBS) with `widths` as `width`; points with `width` and `id`; cameras (lens and film back in millimetres, from tenths of a stage unit); lights (a sphere without a cone a spot of 180°; a dome's image an Image node of the library, named by `op:`).
- **Instances.** A prototype is one value held once, its geometry gathered and the prototypes it shows nested as instances (at most eight deep); an instanceable prim and a point instancer (the instances that show) place it. An instance binding a material the prototype's prims leave unbound names it on the Instance domain.

Animation, skins and blend shapes are not read: a file is read at its start time code (D25). Referenced MaterialX documents and render contexts other than the three are left out and said.

## Export

File ▸ Export writes one layer in the project's units (`metersPerUnit`, `upAxis`): a usda, a usdc, or a usdz holding its textures under `textures/` (the project's embedded files, and files on disk that are pictures a usdz holds by their bytes, PNG, JPEG, OpenEXR or AVIF; any other file an Image node names stays a path, said; a usda or usdc names them as every written document does, the SDK's `format::files::Beside`: a file on disk by its path relative to the layer, an embedded one written into `<stem>_textures/` beside it). The default prim is the one group the items are under, else one named after the file, its materials in its `Looks`.

- **Materials: every authored network** (the owner's ruling for USDZ, 2026-11-07). A network of USD Preview Surface's nodes is written as it is, under `outputs:surface` alone. A MaterialX network goes under `outputs:mtlx:surface`, and an engine's under `outputs:kinogaki:surface` with its standard form under `outputs:mtlx:surface`. A node of Kinogaki's own definitions is written as the MaterialX definition it inherits, its own inputs beside, every one (glTF's texture with its whole sampler as MaterialX's glTF texture holding `minfilter`), so other applications know every shader's `info:id` (`usdchecker --arkit` passes) and Studio reads it back as it was. A material's name and Double Sided are its `customData`'s `kinogaki:name` and `kinogaki:doubleSided`. Beside a MaterialX network or a standard form, a USD Preview Surface is made for other applications: the surface translated as every format translates a material into its own model (the SDK's `MaterialNetwork::translated`: MaterialX's translation graphs, OpenPBR Surface to Standard Surface to USD Preview Surface, and Kinogaki's from glTF PBR and Surface Unlit, folded), each texture left feeding an input carried over as a USD UV Texture of the same file (a texture of Kinogaki's as the MaterialX one it inherits), the arithmetic the translation left between them its scale and bias (found by evaluating that arithmetic with the texture at zero and at one); a normal map into `normal`. What else feeds the surface is left at its value and said. Colors are written in the working space, said by `colorSpace`, USD Preview Surface's in linear Rec.709.
- **Prims**: polygons as a Mesh with its attributes as primvars (`uv` as `st` and the rest as read), double-sided as its materials are, its faces' materials as bindings (GeomSubsets of family `materialBind` where they differ) and its primitive and point groups as GeomSubsets; curves as BasisCurves or NurbsCurves, one prim a kind; points alone as Points; what the host makes polygons of (a B-rep) meshed; lights and cameras; instances as copies of their prototypes, each instance's material bound on its copy.

## The model

`src/usd/`: the format's own model, parser and writer.

### Reading

- **Layers** (`usd::sdf`, `usd::text`, `usd::crate_file`, `usd::package`): usda text; usdc crates of versions 0.0 to 0.15; usdz packages (stored, 64-byte aligned, nested), every entry read in place. A layer is recognized by its signature whatever its extension says. Every count a crate gives is charged to the read's budget before anything is allocated for it, a compressed section claims no more than LZ4 can give (255 bytes a byte), a value several fields share is decoded once, and paths are at most 4096 elements deep, written, compared and dropped without recursion.
- **Composition** (`usd::compose`, after Pcp): sublayers with offsets and rates, references, payloads (with load rules), variants (authored, session and fallback selections), inherits, specializes, relocates, variable expressions, and instancing. A stage opens from a path, from a path with its bytes given (read once by the caller, whose buffer a package's entries are views of), or from bytes in memory (an embedded file), which names no file beside it. Every file on disk is reached through the SDK's resolver (`format::files::Named`: the file's folder and below, regular files, within the read's budget), and only a `.usdz` opens as a package. A layer is opened once by its identity, the canonical path the resolver gives, however it is spelled (a link, another case), and a package read once likewise. List ops (references, payloads, API schemas, targets) and their checks, prim orders and relocates are applied by hashing their items, so their work is linear in their items; an arc's map through its layer stack's relocations is made once, charged to the read's budget and shared by every index mapping through it, and a large set of pairs maps a path through an index of its paths. A layer stack holds each layer once (a sublayer met again is already there, stronger), a prim index at most 65,536 nodes (a diamond of references brings its nodes again at each meeting, as in Pcp), arcs and sublayers nest at most as deep as the read's budget allows, and an expression's string is at most 4096 bytes, each variable evaluated once. The read's budget's cancellation stops composing, flattening and reading the scene. A reference to a MaterialX document (`@look.mtlx@`) is kept for MaterialX's reader instead of composed.
- **The scene** (`usd::scene`): a tree of prims, parents first, each with its local transform, `!resetXformStack!`, visibility and bound material, holding
  - a Mesh with its primvars (the layer's arrays, shared, with interpolation, element size, indices and color space), subdivision tags, GeomSubsets and their bindings;
  - an implicit shape (Cube, Sphere, Cylinder, Cone, Capsule, Plane) as authored, tessellated on request;
  - BasisCurves (linear, Bézier, B-spline, Catmull-Rom; nonperiodic, periodic, pinned) and NurbsCurves; Points;
  - a UsdGeomCamera; a UsdLux light (Rect, Disk, Dome with its image, Distant, Sphere ± ShapingAPI's cone);
  - a PointInstancer placing prototypes; an instanceable prim showing a shared prototype.

  Values are read at one time code. Purposes `proxy` and `guide` load when asked.
- **Materials** (`usd::shade`): every Material prim as its UsdShade network as authored: its terminal outputs per render context (`outputs:surface` USD Preview Surface, `outputs:mtlx:surface` MaterialX, `outputs:kinogaki:surface` an engine's own nodes), its interface inputs, and its Shader and NodeGraph prims with their `info:*`, inputs, outputs, color spaces and connections. Bindings follow UsdShade: full purpose before all-purpose, `strongerThanDescendants`, inherited down the tree.
- **Assets**: the files materials and lights name, a package's entries held as its bytes (no copy), files on disk by path, found by whoever reads them.

### Writing

A scene authors as one layer (`defaultPrim`, `upAxis`, `metersPerUnit`; prims, bindings, materials) and writes as usda, usdc 0.8, or a usdz carrying under `textures/` its held assets and the pictures on disk it names; a loose layer names its assets as they are, writing none beside it (the format names them through `format::files::Beside`). Layers write as `usdcat` prints and writes them.

## Code

| Path | What |
|---|---|
| `src/usd/sdf/` | The data model: tokens, paths, values, list ops, layers of specs |
| `src/usd/text/`, `src/usd/crate_file/`, `src/usd/package/` | usda, usdc, usdz |
| `src/usd/compose/` | Stages: layer stacks, prim indexes, flattening |
| `src/usd/scene/` | The scene model, its reading (`read.rs`) and authoring (`write/`) |
| `src/usd/shade/` | UsdShade networks, read and authored |
| `src/lib.rs` | The package and the format |
| `src/read/` | A file to the standard values: the walk, instances and prototypes (`mod.rs`), geometry, materials, the files they name |
| `src/write/` | The standard values to a file: the items (`mod.rs`), geometry, materials, the USD Preview Surface made for other applications (`preview.rs`), the files written |
| `src/shading.rs`, `src/lights.rs`, `src/space.rs`, `src/names.rs` | What both share: render contexts and shader values; lights and cameras; the stage's units and axes against the project's; names |
| `examples/kusd.rs` | Prints, converts, flattens and times layers; drives the parity checks |
| `tools/parity.py` | Differential checks against OpenUSD's `usdcat` (local) |

## Tests and data

`cargo test` runs the unit tests and two test binaries. The model's (`tests/model/`): composition (`compose.rs`: relocated children and prim orders among it), the scene (`scene.rs`), materials (`shade.rs`), instancing (`instancing.rs`), packages and memory input (`package.rs`), and mutation fuzzing of text, crates and packages (`fuzz.rs`). The format's (`tests/format/`), through the SDK's contract with a color service converting nothing: the fixtures read into the project's units with their primvars, bindings, lights and cameras, instances sharing a prototype, damaged and truncated files and a cancelled read (`import.rs`); the audit's hostile files, made by `crafted.rs` and read as Import reads them, each an error or a warning within a time bound and a small budget (300,000 API schemas, 150,000 references, 20,000 relocates, 40,000 children a reference brings each relocated, a prim order of 100,000 names, a layer named 64 ways), and the export's names and packing (`hostile.rs`); an engine's material written as a usdz and read back as itself with its standard form, a MaterialX material and a USD Preview Surface read back as they were, a glTF material's Kinogaki textures written as MaterialX's and read back as themselves, a material's sides and lengths across units, the contexts each is written in, the Preview Surface made for other applications, a usdz's texture read back as a range of the package (`materials.rs`); polygons, curves, instances, lights, cameras and a dome's image written and read back in any project's units (`export.rs`); the budgets below (`measure.rs`, `hand_run_`). Studio's tree runs the editor's workflow (`workflows/`): a usdz of Spectre's material imports as Spectre's own, exports and imports back. Every fixture under `tests/fixtures/` is written for this repository and is under its licence. Larger corpora are local only: OpenUSD's own test layers through `tools/parity.py`, and Pixar's Kitchen Set (not redistributable) for timing.

## Checks against OpenUSD

`tools/parity.py` runs `kusd` against an OpenUSD v26.08 build's `usdcat` over OpenUSD's own test layers (local only; OpenUSD is an oracle, never a dependency):

| Suite | Agree |
|---|---|
| usda printed as usdcat prints it | 2,018 of 2,056 (2,017 byte for byte); the rest hold attribute splines (31), array edits (4) or strings that are not UTF-8 (2), which are refused, and one path expression not re-anchored |
| usdc printed as usda | 30 of 30, byte for byte |
| usdz root layers printed as usda | 51 of 51, byte for byte |
| usda written as usdc, read back by usdcat | 2,018 of 2,056 (16 that usdcat can't write either) |
| usdc written as usda | 30 of 30 |
| stages flattened (`usdcat --flatten`) | 1,926 of 2,055 (1,925 byte for byte) |
| scenes read and written, read by usdcat | 2,101 of 2,101 written; 174 layers refused (invalid on purpose, or splines and array edits) |

A usdz the format writes of MaterialX and USD Preview Surface materials with packaged textures passes Apple's `usdchecker --arkit` (USD 25.11; `tests/format/materials.rs`'s `hand_run_usdchecker_arkit_passes_materialx_and_preview_surface_usdz`). An engine's own network (`outputs:kinogaki:surface`) fails its shader registry check, which knows no such shader, whether named by `info:id` or by a source type; a usdz keeps it anyway (the owner's ruling).

## Performance

Release builds on an Apple M4 Max (16 cores), best of 10, through `kusd` with one worker per core (`examples/kusd.rs`); the files are read from disk, the times include it:

| Case | Time |
|---|---:|
| An 837 × 837 quad grid (700,569 faces, faceVarying `st`), usdc 8.4 MB: opened and read as a scene | 5.4 ms |
| The grid as usda (51 MB) | 13.2 ms |
| The grid as usdz with a 100 MB texture (against 5.5 ms without) | 15.2 ms |
| The grid authored and written as a usdz file | 12.0 ms |
| Pixar's Kitchen Set: composed, flattened and read as a scene (2,742 prims) | 67 ms |
| Kitchen Set, instanced (114 prototypes) | 53 ms |

`kusd grid <out> 837 [MB]` writes the grid; `kusd scene-time <file>` and `kusd bench <layer>` time it.

Through the format into the standard values and back (`tests/format/measure.rs`, release, best of 10 on workers a thread a core, load average 14 to 22 from other work), against the plan's budgets:

| Case | Time | Budget |
|---|---:|---:|
| The grid's usdc read into the standard values | 21.4 ms | 40 ms |
| The grid's usda | 32.1 ms | 80 ms |
| The grid's usdz with a 100 MB texture, against without | +6.1 ms | +20 ms |
| The grid written from the standard values as a usdz | 46.9 ms | 60 ms |
| Pixar's Kitchen Set imported (2,742 items) | 149.6 ms | 250 ms |

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
