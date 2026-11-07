//! `kusd`: print, convert, flatten and time USD files; the parity checks
//! (`tools/parity.py`) drive it.
//!
//! ```text
//! kusd cat <layer>                print the layer as usda (as usdcat does)
//! kusd write <layer> <out>        write the layer as .usda, .usdc or .usdz
//! kusd flatten <layer>            print the composed stage (usdcat --flatten)
//! kusd scene <file> [out]         summarize the file's scene, or write it out
//! kusd bench <layer> [runs]       time reading and writing every format
//! kusd scene-time <file> [runs]   time opening a stage and reading its scene
//! kusd grid <out> <n> [texture MB] write an n x n quad grid (faceVarying st),
//!                                 with a texture of that size as an asset
//! ```
//!
//! It runs as a host does: it installs workers (one thread per core) for
//! the format's parallel steps.

use std::io::Write;
use std::process::ExitCode;

use kinogaki_studio_api::format::ReadBudget;
use kinogaki_studio_api::format::files::Named;
use kinogaki_studio_api::{FormatOptions, Input, Purpose, Reading, Units};
use kinogaki_studio_api_testing::{Unconverted, workers};
use kinogaki_usd::usd::scene::{self, NodeKind, ReadOptions};
use kinogaki_usd::usd::{self as usd, Result, Stage, StageOptions, crate_file, package, text};

fn main() -> ExitCode {
    workers::install(0);
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let runs = |at: usize| arguments.get(at).and_then(|r| r.parse().ok()).unwrap_or(5);
    let result = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["cat", input] => cat(input),
        ["write", input, output] => read(input).and_then(|layer| usd::write_layer(&layer, output)),
        ["flatten", input] => flatten(input),
        ["scene", input] => summarize(input, None),
        ["scene", input, output] => summarize(input, Some(output)),
        ["bench", input, ..] => bench(input, runs(2)),
        ["scene-time", input, ..] => scene_time(input, runs(2)),
        ["grid", output, n, rest @ ..] => grid(
            output,
            n.parse().unwrap_or(837),
            rest.first().and_then(|m| m.parse().ok()),
        ),
        _ => {
            eprintln!(
                "usage: kusd cat|flatten|scene|bench|scene-time <file> | kusd write|scene <file> <out> | kusd grid <out> <n> [MB]"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kusd: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The layer at `input`, read whole.
fn read(input: &str) -> Result<usd::sdf::LayerData> {
    let bytes = std::fs::read(input).map_err(|e| usd::Error::from(e).in_file(input))?;
    usd::read_layer_bytes(&bytes, &ReadBudget::default()).map_err(|e| e.in_file(input))
}

/// The stage rooted at `input`, reading every file it names.
fn open(input: &str) -> Result<Stage> {
    let reading = Reading::new(Purpose::Objects, Units::default(), &Unconverted);
    let path = std::path::Path::new(input);
    let disk = Named::new(&Input::file(path), &FormatOptions::new(), &reading);
    let options = StageOptions {
        disk: Some(std::sync::Arc::new(disk)),
        ..StageOptions::default()
    };
    Stage::open(path, &options)
}

fn cat(input: &str) -> Result<()> {
    let text = text::write_usda(&read(input)?)?;
    std::io::stdout().lock().write_all(&text)?;
    Ok(())
}

/// Print the composed stage as usda, as `usdcat --flatten` does.
fn flatten(input: &str) -> Result<()> {
    let mut stage = open(input)?;
    let layer = stage.flatten()?;
    for warning in stage.warnings() {
        eprintln!("kusd: warning: {warning}");
    }
    std::io::stdout()
        .lock()
        .write_all(&text::write_usda(&layer)?)?;
    Ok(())
}

/// Summarize the scene of `input`; with `output`, write it there.
fn summarize(input: &str, output: Option<&str>) -> Result<()> {
    let mut stage = open(input)?;
    let scene = scene::read(&mut stage, &ReadOptions::default())?;
    for warning in stage.warnings().iter().chain(&scene.warnings) {
        eprintln!("warning: {warning}");
    }
    for (index, node) in scene.nodes.iter().enumerate() {
        let what = match &node.kind {
            NodeKind::Mesh(mesh) => format!(
                "mesh points={} faces={}",
                mesh.points.len(),
                mesh.face_vertex_counts.len()
            ),
            NodeKind::Instancer(instancer) => format!("instancer instances={}", instancer.len()),
            NodeKind::Group if node.instance.is_some() => "instance".to_owned(),
            other => format!("{other:?}")
                .split(['(', ' '])
                .next()
                .unwrap_or("")
                .to_lowercase(),
        };
        println!("{} {what}", scene.path(index));
    }
    println!(
        "{} nodes, {} prototypes, {} materials, {} assets",
        scene.nodes.len(),
        scene.prototypes.len(),
        scene.materials.len(),
        scene.assets.len()
    );
    match output {
        Some(output) => scene::write_file(&scene, output),
        None => Ok(()),
    }
}

/// Best-of-`runs` wall time of `job` in milliseconds, and its last result.
fn best<T>(runs: usize, mut job: impl FnMut() -> Result<T>) -> Result<(f64, T)> {
    let mut best = f64::MAX;
    let mut last = None;
    for _ in 0..runs.max(1) {
        let started = std::time::Instant::now();
        let result = job()?;
        best = best.min(started.elapsed().as_secs_f64() * 1e3);
        last = Some(result);
    }
    Ok((best, last.expect("at least one run")))
}

/// Time reading and writing the layer in every format (files read whole
/// first: the times exclude file I/O).
fn bench(input: &str, runs: usize) -> Result<()> {
    let bytes = std::fs::read(input)?;
    let (input_read, layer) = best(runs, || {
        usd::read_layer_bytes(&bytes, &ReadBudget::default())
    })?;
    let (usda_write, usda) = best(runs, || text::write_usda(&layer))?;
    let (usda_read, _) = best(runs, || text::read_usda(&usda, &ReadBudget::default()))?;
    let (usdc_write, usdc) = best(runs, || crate_file::write_usdc(&layer))?;
    let (usdc_read, _) = best(runs, || {
        crate_file::read_usdc(&usdc, &ReadBudget::default())
    })?;
    let (usdz_write, usdz) = best(runs, || package::write_usdz(&layer, "root.usdc", &[]))?;
    let (usdz_read, _) = best(runs, || package::read_usdz(&usdz, &ReadBudget::default()))?;
    println!("| Case | Time (best of {runs}) | Bytes |");
    println!("|---|---:|---:|");
    for (name, ms, size) in [
        ("input read", input_read, bytes.len()),
        ("usda read", usda_read, usda.len()),
        ("usda write", usda_write, usda.len()),
        ("usdc read", usdc_read, usdc.len()),
        ("usdc write", usdc_write, usdc.len()),
        ("usdz read", usdz_read, usdz.len()),
        ("usdz write", usdz_write, usdz.len()),
    ] {
        println!("| {name} | {ms:.1} ms | {size} |");
    }
    Ok(())
}

/// Time opening the stage, reading its scene, and writing it as a usdz
/// (authored, packaged in memory, and written to a file with its assets).
fn scene_time(input: &str, runs: usize) -> Result<()> {
    let (opened, _) = best(runs, || open(input))?;
    let (read, scene) = best(runs, || {
        scene::read(&mut open(input)?, &ReadOptions::default())
    })?;
    let (author, layer) = best(runs, || scene::author(&scene))?;
    let (usdz, _) = best(runs, || package::write_usdz(&layer, "root.usdc", &[]))?;
    let file = std::env::temp_dir().join(format!("kusd-{}.usdz", std::process::id()));
    let (written, _) = best(runs, || scene::write_file(&scene, &file))?;
    std::fs::remove_file(&file)?;
    println!(
        "open {opened:.1} ms, open + scene {read:.1} ms, author {author:.1} ms, usdz {usdz:.1} ms, usdz file {written:.1} ms (best of {runs})"
    );
    Ok(())
}

/// Write an `n` x `n` quad grid of centimetre cells in the XY plane, a
/// small height on each point (so its text holds full floats), its `st`
/// faceVarying and indexed, bound to a material whose texture is
/// `megabytes` of bytes.
fn grid(output: &str, n: usize, megabytes: Option<usize>) -> Result<()> {
    use kinogaki_usd::usd::scene::{
        Asset, AssetSource, Interpolation, Mesh, Node, Primvar, Scene, Subdivision,
    };
    use kinogaki_usd::usd::sdf::{Array, Path, Token, Value};
    use kinogaki_usd::usd::shade::{Material, Port, ShadeKind, ShadeNode, Source};
    use std::sync::Arc;
    let side = n + 1;
    let height = |i: usize| (i.wrapping_mul(2_654_435_761) % 10_007) as f32 * 1.0e-5;
    let points = (0..side * side)
        .map(|i| {
            [
                (i % side) as f32 * 0.01,
                (i / side) as f32 * 0.01,
                height(i),
            ]
        })
        .collect();
    let mut indices = Vec::with_capacity(n * n * 4);
    for row in 0..n {
        for column in 0..n {
            let corner = (row * side + column) as i32;
            let side = side as i32;
            indices.extend([corner, corner + 1, corner + side + 1, corner + side]);
        }
    }
    let st = Primvar {
        name: Token::new("st"),
        type_name: Token::new("texCoord2f[]"),
        interpolation: Interpolation::FaceVarying,
        element_size: 1,
        values: Value::Array(Array::Vec2f(Arc::new(vec![
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [0.0, 1.0],
        ]))),
        indices: Some(Arc::new((0..n * n * 4).map(|i| (i % 4) as i32).collect())),
        color_space: None,
    };
    let mesh = Mesh {
        points: Arc::new(points),
        face_vertex_counts: Arc::new(vec![4; n * n]),
        face_vertex_indices: Arc::new(indices),
        left_handed: false,
        primvars: vec![st],
        subdivision: Subdivision::default(),
        subsets: Vec::new(),
        double_sided: false,
    };
    let mut scene = Scene::default();
    scene.nodes.push(Node::group("World", None));
    let mut plane = Node::group("Grid", Some(0));
    plane.type_name = Token::new("Mesh");
    plane.kind = NodeKind::Mesh(Box::new(mesh));
    if let Some(megabytes) = megabytes {
        let path = Path::parse("/World/Looks/Checker").expect("a path");
        let mut material = Material::new(path.clone());
        let file = Token::new("checker.png");
        let mut texture = ShadeNode {
            path: path.child("Texture"),
            kind: ShadeKind::Shader,
            parent: None,
            info: vec![Port::new(
                "id",
                "token",
                Some(Value::Token(Token::new("UsdUVTexture"))),
            )],
            inputs: vec![Port::new("file", "asset", Some(Value::Asset(file.clone())))],
            outputs: vec![Port::new("rgb", "float3", None)],
        };
        texture.info[0].uniform = true;
        let mut surface = Port::new("surface", "token", None);
        surface.sources.push(Source {
            node: Some(1),
            port: Token::new("surface"),
            output: true,
        });
        let mut diffuse = Port::new("diffuseColor", "color3f", None);
        diffuse.sources.push(Source {
            node: Some(0),
            port: Token::new("rgb"),
            output: true,
        });
        let mut preview = ShadeNode {
            path: path.child("Preview"),
            kind: ShadeKind::Shader,
            parent: None,
            info: texture.info.clone(),
            inputs: vec![diffuse],
            outputs: vec![Port::new("surface", "token", None)],
        };
        preview.info[0].value = Some(Value::Token(Token::new("UsdPreviewSurface")));
        material.nodes = vec![texture, preview];
        material.outputs.push(surface);
        scene.materials.push(material);
        plane.material = Some(0);
        let bytes = (0..megabytes << 20)
            .map(|i| (i * 7 + i / 4096) as u8)
            .collect::<Vec<u8>>();
        scene.assets.push(Asset {
            path: file.clone(),
            authored: file,
            source: AssetSource::Bytes(bytes.into()),
        });
    }
    scene.nodes.push(plane);
    scene::write_file(&scene, output)
}
