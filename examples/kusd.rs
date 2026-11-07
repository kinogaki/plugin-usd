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
//! ```
//!
//! It runs as a host does: it installs workers (one thread per core) for
//! the format's parallel steps.

use std::io::Write;
use std::ops::Range;
use std::panic::Location;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use kinogaki_studio_api::geometry::parallel::{self, Workers};
use kinogaki_usd::usd::scene::{self, NodeKind, ReadOptions};
use kinogaki_usd::usd::{self as usd, Result, Stage, crate_file, package, text};

fn main() -> ExitCode {
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    parallel::install(Box::leak(Box::new(Pool { threads })));
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let runs = |at: usize| arguments.get(at).and_then(|r| r.parse().ok()).unwrap_or(5);
    let result = match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["cat", input] => cat(input),
        ["write", input, output] => {
            usd::read_layer(input).and_then(|layer| usd::write_layer(&layer, output))
        }
        ["flatten", input] => flatten(input),
        ["scene", input] => summarize(input, None),
        ["scene", input, output] => summarize(input, Some(output)),
        ["bench", input, ..] => bench(input, runs(2)),
        ["scene-time", input, ..] => scene_time(input, runs(2)),
        _ => {
            eprintln!(
                "usage: kusd cat|flatten|scene|bench|scene-time <file> | kusd write|scene <file> <out>"
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

fn cat(input: &str) -> Result<()> {
    let text = text::write_usda(&usd::read_layer(input)?)?;
    std::io::stdout().lock().write_all(&text)?;
    Ok(())
}

/// Print the composed stage as usda, as `usdcat --flatten` does.
fn flatten(input: &str) -> Result<()> {
    let mut stage = Stage::open(input)?;
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
    let mut stage = Stage::open(input)?;
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
    let (input_read, layer) = best(runs, || usd::read_layer_bytes(&bytes))?;
    let (usda_write, usda) = best(runs, || text::write_usda(&layer))?;
    let (usda_read, _) = best(runs, || text::read_usda(&usda))?;
    let (usdc_write, usdc) = best(runs, || crate_file::write_usdc(&layer))?;
    let (usdc_read, _) = best(runs, || crate_file::read_usdc(&usdc))?;
    let (usdz_write, usdz) = best(runs, || package::write_usdz(&layer, "root.usdc", &[]))?;
    let (usdz_read, _) = best(runs, || package::read_usdz(&usdz))?;
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

/// Time opening the stage, reading its scene, and writing it as a usdz.
fn scene_time(input: &str, runs: usize) -> Result<()> {
    let (open, _) = best(runs, || Stage::open(input))?;
    let (read, scene) = best(runs, || {
        scene::read(&mut Stage::open(input)?, &ReadOptions::default())
    })?;
    let (author, layer) = best(runs, || scene::author(&scene))?;
    let (usdz, _) = best(runs, || package::write_usdz(&layer, "root.usdc", &[]))?;
    println!(
        "open {open:.1} ms, open + scene {read:.1} ms, author {author:.1} ms, usdz {usdz:.1} ms (best of {runs})"
    );
    Ok(())
}

/// The workers a host installs: the calling thread and scoped helpers, one
/// per core, taking ranges from a shared counter.
struct Pool {
    threads: usize,
}

impl Workers for Pool {
    fn run(
        &self,
        _site: &'static Location<'static>,
        len: usize,
        minimum: usize,
        work: &(dyn Fn(Range<usize>) -> bool + Sync),
    ) {
        let step = minimum.max(1);
        let threads = self.threads.min(len.div_ceil(step));
        if threads <= 1 {
            if len > 0 {
                work(0..len);
            }
            return;
        }
        let (next, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let take = || {
            while !stop.load(Ordering::Relaxed) {
                let start = next.fetch_add(step, Ordering::Relaxed);
                if start >= len {
                    break;
                }
                if !work(start..(start + step).min(len)) {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        };
        std::thread::scope(|scope| {
            for _ in 1..threads {
                scope.spawn(take);
            }
            take();
        });
    }
}
