//! Malformed input: mutated text, crates and packages must fail with an
//! error, never panic, and stay within their limits.

use kinogaki_usd::usd::package::Bytes;
use kinogaki_usd::usd::scene::{self, ReadOptions};
use kinogaki_usd::usd::{Stage, StageOptions};

/// xorshift64*: deterministic, so a failure reproduces.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit.max(1) as u64) as usize
    }
}

/// `bytes` with a few random edits: flipped bytes, deletions, duplicated
/// runs, truncation, or special bytes.
fn mutate(bytes: &[u8], random: &mut Random) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for _ in 0..1 + random.below(4) {
        if out.is_empty() {
            break;
        }
        let at = random.below(out.len());
        match random.below(6) {
            0 => out[at] ^= 1 << random.below(8),
            1 => {
                let end = (at + random.below(16)).min(out.len());
                out.drain(at..end);
            }
            2 => {
                let end = (at + random.below(32)).min(out.len());
                let run = out[at..end].to_vec();
                out.splice(at..at, run);
            }
            3 => out.truncate(at),
            4 => out[at] = [0, 0xFF, b'[', b'(', b'{', b'"', b'@', b'<', b'\\'][random.below(9)],
            _ => out[at] = random.next() as u8,
        }
    }
    out
}

fn survive(bytes: &[u8]) {
    if let Ok(layer) = kinogaki_usd::usd::read_layer_bytes(bytes) {
        // Whatever reads must also print, and read as a scene that writes.
        let _ = kinogaki_usd::usd::text::write_usda(&layer);
        let _ = kinogaki_usd::usd::crate_file::write_usdc(&layer);
        let options = StageOptions::default();
        if let Ok(mut stage) = Stage::open_memory("fuzz.usd", Bytes::from(bytes.to_vec()), &options)
            && let Ok(scene) = scene::read(&mut stage, &ReadOptions::default())
        {
            let _ = scene::author(&scene);
        }
    }
}

fn layer() -> kinogaki_usd::usd::sdf::LayerData {
    kinogaki_usd::usd::text::read_usda(include_bytes!("../fixtures/cube.usda")).unwrap()
}

#[test]
fn mutated_text_never_panics() {
    let text = include_bytes!("../fixtures/cube.usda");
    let mut random = Random(0x1234_5678);
    for _ in 0..20_000 {
        survive(&mutate(text, &mut random));
    }
}

#[test]
fn mutated_crates_never_panic() {
    let crate_bytes = kinogaki_usd::usd::crate_file::write_usdc(&layer()).unwrap();
    let mut random = Random(0x9876_5432);
    for _ in 0..20_000 {
        survive(&mutate(&crate_bytes, &mut random));
    }
}

#[test]
fn mutated_packages_never_panic() {
    let package = kinogaki_usd::usd::package::write_usdz(&layer(), "root.usdc", &[]).unwrap();
    let mut random = Random(0x5555_AAAA);
    for _ in 0..10_000 {
        survive(&mutate(&package, &mut random));
    }
}

#[test]
fn deep_nesting_is_refused() {
    let mut text = b"#usda 1.0\n(\n    customLayerData = ".to_vec();
    for _ in 0..10_000 {
        text.extend_from_slice(b"{ dictionary a = ");
    }
    let error = kinogaki_usd::usd::text::read_usda(&text).unwrap_err();
    assert!(error.position.is_some());
    let mut prims = b"#usda 1.0\n".to_vec();
    for _ in 0..10_000 {
        prims.extend_from_slice(b"def \"a\" {\n");
    }
    assert!(kinogaki_usd::usd::text::read_usda(&prims).is_err());
}

#[test]
fn errors_name_the_line_and_column() {
    let error =
        kinogaki_usd::usd::text::read_usda(b"#usda 1.0\ndef Mesh \"a\"\n{\n    int x = 1.5.\n}\n")
            .unwrap_err();
    assert_eq!(error.position.map(|(line, _)| line), Some(4));
    assert!(kinogaki_usd::usd::read_layer_bytes(b"nothing").is_err());
}
