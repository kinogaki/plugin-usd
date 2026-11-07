//! Composition on our own fixtures (tests/fixtures/compose): strength order,
//! variant selection, payload loading, layer offsets, instancing, cycles.

use kinogaki_studio_api::format::ReadBudget;
use kinogaki_studio_api_testing::format::Scratch;
use std::path::PathBuf;

use kinogaki_usd::usd::sdf::{LayerData, Path, Value, field};
use kinogaki_usd::usd::{FlattenOptions, StageOptions};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/compose")
        .join(name)
}

fn p(text: &str) -> Path {
    Path::parse(text).unwrap()
}

fn text(layer: &LayerData, path: &str) -> Option<String> {
    match layer.get(&p(path), field::DEFAULT) {
        Some(Value::String(text)) => Some(text.to_string()),
        _ => None,
    }
}

fn flat(name: &str, options: &StageOptions) -> LayerData {
    let mut stage = crate::common::open_with(fixture(name), options).unwrap();
    stage
        .flatten_with(FlattenOptions {
            comment: false,
            instancing: false,
        })
        .unwrap()
}

#[test]
fn opinions_follow_livrps() {
    let layer = flat("livrps_root.usda", &StageOptions::default());
    // Local, then inherits beat variants, references, payloads, specializes.
    assert_eq!(text(&layer, "/Shot.local").as_deref(), Some("local"));
    assert_eq!(text(&layer, "/Shot.winner").as_deref(), Some("inherit"));
    assert_eq!(text(&layer, "/Shot.inherited").as_deref(), Some("inherit"));
    assert_eq!(text(&layer, "/Shot.color").as_deref(), Some("red"));
    assert_eq!(
        text(&layer, "/Shot.referenced").as_deref(),
        Some("reference")
    );
    assert_eq!(text(&layer, "/Shot.loaded").as_deref(), Some("payload"));
    assert_eq!(
        text(&layer, "/Shot.specialized").as_deref(),
        Some("specialize")
    );
    // The referenced prim's children come along.
    let Some(Value::Array(points)) = layer.get(&p("/Shot/Geom.points"), field::DEFAULT) else {
        panic!("no points");
    };
    assert_eq!(points.len(), 3);
    // Composition arcs are resolved, not copied.
    let shot = layer.spec(&p("/Shot")).unwrap();
    assert!(shot.get(field::REFERENCES).is_none());
    assert!(shot.get(field::INHERIT_PATHS).is_none());
}

#[test]
fn session_selections_and_payload_rules_apply() {
    let options = StageOptions {
        variants: vec![p("/Shot{look=blue}")],
        ..StageOptions::default()
    };
    let layer = flat("livrps_root.usda", &options);
    assert_eq!(text(&layer, "/Shot.color").as_deref(), Some("blue"));
    // A variant is weaker than an inherit.
    assert_eq!(text(&layer, "/Shot.winner").as_deref(), Some("inherit"));

    let options = StageOptions {
        load_payloads: vec!["-".to_owned()],
        ..StageOptions::default()
    };
    let layer = flat("livrps_root.usda", &options);
    assert_eq!(text(&layer, "/Shot.loaded"), None);
    assert_eq!(
        text(&layer, "/Shot.referenced").as_deref(),
        Some("reference")
    );
}

#[test]
fn sublayer_offsets_move_samples() {
    let layer = flat("offsets_root.usda", &StageOptions::default());
    let Some(Value::TimeSamples(samples)) = layer.get(&p("/Animated.value"), field::TIME_SAMPLES)
    else {
        panic!("no samples");
    };
    let times: Vec<f64> = samples.iter().map(|(time, _)| *time).collect();
    // stage time = 10 + 2 * layer time
    assert_eq!(times, [10.0, 20.0]);
}

#[test]
fn cycles_and_missing_layers_are_reported() {
    let mut stage = crate::common::open(fixture("cycle_a.usda")).unwrap();
    let layer = stage.flatten().unwrap();
    assert!(layer.has_spec(&p("/A")));
    assert!(
        stage
            .warnings()
            .iter()
            .any(|warning| warning.contains("Cycle")),
        "{:?}",
        stage.warnings()
    );

    let mut stage = crate::common::open(fixture("sublayer_cycle.usda")).unwrap();
    let layer = stage.flatten().unwrap();
    assert!(layer.has_spec(&p("/Here")));
    let warnings = stage.warnings();
    assert!(
        warnings.iter().any(|warning| warning.contains("cycle")),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("missing.usda")),
        "{warnings:?}"
    );
}

#[test]
fn instances_share_prototypes() {
    let mut stage = crate::common::open(fixture("instances.usda")).unwrap();
    let layer = stage.flatten().unwrap();
    assert!(layer.has_spec(&p("/Flattened_Prototype_1/Part")));
    for instance in ["/One", "/Two"] {
        let Some(Value::ReferenceListOp(list)) = layer.get(&p(instance), field::REFERENCES) else {
            panic!("{instance} has no reference");
        };
        assert_eq!(list.added[0].prim, p("/Flattened_Prototype_1"));
        assert!(!layer.has_spec(&p(&format!("{instance}/Part"))));
    }
    // Expanded, each instance holds its own copy.
    let layer = flat("instances.usda", &StageOptions::default());
    assert!(layer.has_spec(&p("/One/Part")) && layer.has_spec(&p("/Two/Part")));
}

#[test]
fn stage_metadata_reads_the_root_layer() {
    let stage = crate::common::open(fixture("livrps_root.usda")).unwrap();
    assert_eq!(stage.up_axis().as_str(), "Z");
    assert_eq!(stage.meters_per_unit(), 1.0);
    assert_eq!(stage.time_codes_per_second(), 24.0);
    assert!(!stage.is_single_layer());
    assert!(crate::common::open(fixture("nothing_here.usda")).is_err());
}

#[test]
fn packaged_layers_compose() {
    let directory = Scratch::new("usd-compose");
    let model = kinogaki_usd::usd::text::read_usda(
        b"#usda 1.0\n(\n    defaultPrim = \"M\"\n)\n\ndef \"M\"\n{\n    custom int inside = 7\n}\n",
        &ReadBudget::default(),
    )
    .unwrap();
    let package = kinogaki_usd::usd::package::write_usdz(&model, "model.usdc", &[]).unwrap();
    std::fs::write(directory.join("model.usdz"), package).unwrap();
    std::fs::write(
        directory.join("root.usda"),
        "#usda 1.0\n\ndef \"Use\" (\n    references = @./model.usdz@\n)\n{\n}\n",
    )
    .unwrap();
    let mut stage = crate::common::open(directory.join("root.usda")).unwrap();
    let layer = stage.flatten().unwrap();
    assert_eq!(
        layer.get(&p("/Use.inside"), field::DEFAULT),
        Some(&Value::Int(7))
    );
}

#[test]
fn deep_arc_chains_fail_cleanly() {
    let scratch = Scratch::new("deep");
    // 2000 layers, each referencing the next: past the nesting limit.
    for at in 0..2000 {
        let body = format!(
            "#usda 1.0\n\ndef \"P\" (\n    references = @./l{}.usda@</P/C>\n)\n{{\n    def \"C\"\n    {{\n    }}\n}}\n",
            at + 1
        );
        std::fs::write(scratch.join(format!("l{at}.usda")), body).unwrap();
    }
    let result = std::thread::Builder::new()
        .stack_size(kinogaki_studio_api_testing::IMPORT_STACK)
        .spawn({
            let root = scratch.join("l0.usda");
            move || {
                crate::common::open(root)
                    .and_then(|mut stage| stage.flatten())
                    .map(|layer| layer.len())
            }
        })
        .unwrap()
        .join()
        .expect("no panic or overflow");
    let error = result.expect_err("past the nesting limit");
    assert_eq!(error.kind, kinogaki_usd::usd::ErrorKind::Limit);
    // 400 sublayers deep: past the sublayer limit.
    for at in 0..400 {
        let body = format!("#usda 1.0\n(\n    subLayers = [@./s{}.usda@]\n)\n", at + 1);
        std::fs::write(scratch.join(format!("s{at}.usda")), body).unwrap();
    }
    assert!(crate::common::open(scratch.join("s0.usda")).is_err());
}

#[test]
fn mutated_layers_never_panic() {
    let scratch = Scratch::new("mutated");
    let sources: Vec<Vec<u8>> = [
        "livrps_root.usda",
        "livrps_ref.usda",
        "instances.usda",
        "offsets_root.usda",
    ]
    .iter()
    .map(|name| std::fs::read(fixture(name)).unwrap())
    .collect();
    for name in ["livrps_ref.usda", "livrps_payload.usda", "offsets_sub.usda"] {
        std::fs::copy(fixture(name), scratch.join(name)).unwrap();
    }
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = b"<>[]{}()@\"=;,./ \nabcXYZ0123456789-:`$";
    for round in 0..300 {
        let mut bytes = sources[round % sources.len()].clone();
        for _ in 0..1 + next() % 6 {
            let at = (next() as usize) % bytes.len();
            match next() % 3 {
                0 => bytes[at] = alphabet[(next() as usize) % alphabet.len()],
                1 => {
                    bytes.remove(at);
                }
                _ => bytes.insert(at, alphabet[(next() as usize) % alphabet.len()]),
            }
        }
        let root = scratch.join("root.usda");
        std::fs::write(&root, &bytes).unwrap();
        if let Ok(mut stage) = crate::common::open(&root) {
            let _ = stage.flatten();
        }
    }
}

#[test]
fn relocates_rename_move_and_bring_children() {
    // Root references Model: its A is renamed C, its B moved under Other,
    // and Other's own D moved here as E.
    let layer = r#"#usda 1.0
(
    relocates = {
        </Root/A>: </Root/C>,
        </Root/B>: </Other/B>,
        </Other/D>: </Root/E>
    }
)
def Xform "Root" (
    references = </Model>
)
{
}
def Xform "Other" (
    references = </Spare>
)
{
}
def Xform "Model"
{
    def Xform "Z" {}
    def Xform "A" {}
    def Xform "B" {}
}
def Xform "Spare"
{
    def Xform "D" {}
}
"#;
    let mut stage = crate::common::stage_named("relocated.usda", layer.as_bytes());
    let names = |stage: &mut kinogaki_usd::usd::Stage, path: &str| -> Vec<String> {
        let names = stage.child_names(&p(path)).unwrap();
        names.iter().map(|n| n.as_str().to_owned()).collect()
    };
    assert_eq!(names(&mut stage, "/Root"), ["Z", "C", "E"]);
    assert_eq!(names(&mut stage, "/Other"), ["B"]);
}

#[test]
fn a_prim_order_takes_the_runs_its_names_start() {
    let layer = r#"#usda 1.0
def Xform "Root"
{
    reorder nameChildren = ["d", "b"]
    def Xform "a" {}
    def Xform "b" {}
    def Xform "c" {}
    def Xform "d" {}
    def Xform "e" {}
}
"#;
    let mut stage = crate::common::stage_named("ordered.usda", layer.as_bytes());
    let names: Vec<String> = (stage.child_names(&p("/Root")).unwrap().iter())
        .map(|n| n.as_str().to_owned())
        .collect();
    assert_eq!(names, ["a", "d", "e", "b", "c"]);
}
