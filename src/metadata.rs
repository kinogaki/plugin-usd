//! A stage's metadata no item holds ([`Scene::default_prim`],
//! [`Scene::metadata`]) as the format's options (`Imported::metadata`), and
//! back for a write of the document read from it (`Writing::metadata`):
//! `defaultPrim`, `documentation` (usda's `doc`) and `renderSettingsPrimPath`
//! as text, the time codes and their rates as numbers, `customLayerData` as
//! the usda text of a layer holding it alone, within the read's budget (a
//! dictionary a file shares among its entries is text of every one). The
//! units are the project's.
use crate::usd::scene::Scene;
use crate::usd::sdf::{LayerData, Token, Value};
use crate::usd::text::{read_usda, write_usda};
use kinogaki_studio_api::FormatOptions;
use kinogaki_studio_api::format::ReadBudget;
use kinogaki_studio_api::params::Value as Held;

/// The default prim's name.
pub(crate) const DEFAULT_PRIM: &str = "defaultPrim";
/// The field naming the render settings prim, written only while it names
/// a prim written.
pub(crate) const RENDER_SETTINGS: &str = "renderSettingsPrimPath";

/// Bytes of text an entry of a dictionary takes at least.
const ENTRY: u64 = 16;

/// What `scene` says of itself, as options, a dictionary's text within
/// `budget`; one past it left out, said in `said`.
pub(crate) fn read(scene: &Scene, budget: &ReadBudget, said: &mut Vec<String>) -> FormatOptions {
    let mut out = FormatOptions::new();
    if let Some(name) = &scene.default_prim {
        out.insert(
            DEFAULT_PRIM.to_owned(),
            Held::Text(name.as_str().to_owned()),
        );
    }
    for (name, value) in &scene.metadata {
        let held = match value {
            Value::Token(text) | Value::String(text) => Held::Text(text.as_str().to_owned()),
            Value::Dictionary(_) => {
                let most = budget.limits().allocation / ENTRY;
                let held = entries(value, most);
                if held > most || budget.allocate(held * ENTRY).is_err() {
                    said.push(format!(
                        "{name}: more than the read's budget holds; left out"
                    ));
                    continue;
                }
                let mut alone = LayerData::new();
                alone.root_mut().set(name.clone(), value.clone());
                match write_usda(&alone) {
                    Ok(text) => Held::Text(String::from_utf8_lossy(&text).into_owned()),
                    Err(_) => continue,
                }
            }
            other => match other.as_f64() {
                Some(number) => Held::Number(number),
                None => continue,
            },
        };
        out.insert(name.as_str().to_owned(), held);
    }
    out
}

/// The entries `value` holds, a dictionary held twice counted twice;
/// counting stops past `most`.
fn entries(value: &Value, most: u64) -> u64 {
    let Value::Dictionary(held) = value else {
        return 0;
    };
    let mut counted = 0u64;
    for inner in held.values() {
        counted += 1 + entries(inner, most.saturating_sub(counted));
        if counted > most {
            break;
        }
    }
    counted
}

/// The layer metadata `metadata` holds of the fields a scene keeps, as the
/// layer spells them, the render settings prim's path aside; one that
/// doesn't read said in `said`.
pub(crate) fn fields(metadata: &FormatOptions, said: &mut Vec<String>) -> Vec<(Token, Value)> {
    let mut out = Vec::new();
    for name in crate::usd::scene::METADATA {
        let value = match (name, metadata.get(name)) {
            (_, None) | (RENDER_SETTINGS, _) => continue,
            ("customLayerData", Some(Held::Text(text))) => {
                let read = read_usda(text.as_bytes(), &ReadBudget::default());
                match read.ok().and_then(|layer| layer.root().get(name).cloned()) {
                    Some(dictionary) => dictionary,
                    None => {
                        said.push(format!("{name} didn't read; left out"));
                        continue;
                    }
                }
            }
            (_, Some(Held::Text(text))) => Value::String(Token::new(text)),
            (_, Some(Held::Number(number))) => Value::Double(*number),
            (_, Some(_)) => continue,
        };
        out.push((Token::new(name), value));
    }
    out
}
