// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Layer stacks, after `PcpLayerStack::_BuildLayerStack`
//! (`pxr/usd/pcp/layerStack.cpp`): a session layer's tree (when there is
//! one), then the root layer's, each layer followed by its sublayers depth
//! first, strongest first. Every layer carries the offset mapping its times
//! into the root's: its sublayer offset composed with its parent's, and
//! scaled when its timeCodesPerSecond differs. A sublayer that is missing or
//! already an ancestor (a cycle) is skipped with an error.
//!
//! A stack's expression variables are the root layer's
//! `expressionVariables`, the session layer's over them, and those of the
//! layer stack that references it over both; a sublayer path may be an
//! expression over them. Its relocations come from its layers'
//! `layerRelocates` (`relocations.rs`).

use super::Problems;
use super::assets;
use super::expressions::{evaluate_to_string, is_expression};
use super::layers::Layers;
use super::relocations::{Relocate, compute_relocations};
use crate::usd::error::{Error, Result};
use crate::usd::sdf::{Dictionary, LayerOffset, Token, Value, field};

/// The deepest sublayer nesting accepted.
const MOST_SUBLAYER_DEPTH: usize = 256;

/// One layer stack.
#[derive(Clone, Debug)]
pub struct LayerStack {
    /// The root layer.
    pub root: usize,
    /// The session layer, if any.
    pub session: Option<usize>,
    /// Layers, strongest first.
    pub layers: Vec<usize>,
    /// Each layer's offset into the root's time.
    pub offsets: Vec<LayerOffset>,
    /// The stack's time codes per second.
    pub time_codes_per_second: f64,
    /// The stack's relocations.
    pub relocates: Vec<Relocate>,
    /// The expression variables.
    pub variables: Dictionary,
    /// The stack that last changed the variables (none: the root stack).
    pub source: Option<usize>,
    /// The stack whose variables override this one's (part of its identity).
    pub overrides: Option<usize>,
}

impl LayerStack {
    /// Whether the stack has relocations.
    pub fn has_relocates(&self) -> bool {
        !self.relocates.is_empty()
    }

    /// The incremental source relocated to `target`, if any.
    pub fn relocated_to(&self, target: &crate::usd::sdf::Path) -> Option<&crate::usd::sdf::Path> {
        if target.is_empty() {
            return None;
        }
        self.relocates
            .iter()
            .find(|relocate| relocate.target == *target)
            .map(|relocate| &relocate.source)
    }

    /// Whether `source` is relocated (incrementally).
    pub fn relocates_source(&self, source: &crate::usd::sdf::Path) -> bool {
        self.relocates
            .iter()
            .any(|relocate| relocate.source == *source)
    }
}

/// Every layer stack a composition has built, each once.
#[derive(Default)]
pub struct Stacks {
    stacks: Vec<LayerStack>,
}

impl Stacks {
    /// Stack `index`.
    pub fn stack(&self, index: usize) -> &LayerStack {
        &self.stacks[index]
    }

    /// The layer stack of root layer `root` (and `session`) whose variables
    /// `overrides` overrides, built now if new.
    pub fn find_or_build(
        &mut self,
        layers: &mut Layers,
        root: usize,
        session: Option<usize>,
        overrides: Option<usize>,
        problems: &mut Problems,
    ) -> Result<usize> {
        if let Some(at) = self.stacks.iter().position(|stack| {
            stack.root == root && stack.session == session && stack.overrides == overrides
        }) {
            return Ok(at);
        }
        let mut stack = LayerStack {
            root,
            session,
            layers: Vec::new(),
            offsets: Vec::new(),
            time_codes_per_second: layers.time_codes_per_second(root),
            relocates: Vec::new(),
            variables: Dictionary::new(),
            source: None,
            overrides,
        };
        self.compose_variables(layers, &mut stack);
        let mut seen = Vec::new();
        let mut root_offset = LayerOffset::IDENTITY;
        if let Some(owner) = session {
            let session_rate = layers.time_codes_per_second(owner);
            let mut session_offset = LayerOffset::IDENTITY;
            if session_governs(layers, owner, root) {
                stack.time_codes_per_second = session_rate;
                root_offset.scale = session_rate / layers.time_codes_per_second(root);
            } else {
                session_offset.scale = stack.time_codes_per_second / session_rate;
            }
            build(
                layers,
                owner,
                session_offset,
                session_rate,
                None,
                &mut stack,
                &mut seen,
                problems,
            )?;
        }
        // The session tree's first sessionOwner reorders owned sublayers.
        let session_owner = stack
            .layers
            .iter()
            .find_map(|&layer| layers.token_field(layer, "sessionOwner"));
        let rate = layers.time_codes_per_second(root);
        build(
            layers,
            root,
            root_offset,
            rate,
            session_owner.as_ref(),
            &mut stack,
            &mut seen,
            problems,
        )?;
        compute_relocations(layers, &mut stack, problems);
        self.stacks.push(stack);
        Ok(self.stacks.len() - 1)
    }

    /// The stack's expression variables: later sources win; the source
    /// stays the overriding stack's when nothing changed.
    fn compose_variables(&self, layers: &Layers, stack: &mut LayerStack) {
        let mut made = Dictionary::new();
        for layer in std::iter::once(stack.root).chain(stack.session) {
            if let Some(found) = layers
                .field(layer, field::EXPRESSION_VARIABLES)
                .and_then(Value::as_dictionary)
            {
                over(&mut made, found);
            }
        }
        // The root layer stack's own variables are the base of the others.
        if self.stacks.is_empty() {
            stack.variables = made;
            return;
        }
        let overrides = stack.overrides.unwrap_or(0);
        let base = &self.stacks[overrides];
        over(&mut made, &base.variables);
        stack.source = if made != base.variables {
            Some(self.stacks.len())
        } else {
            base.source
        };
        stack.variables = made;
    }
}

/// Put `stronger`'s entries over `into` (`VtDictionaryOver`).
fn over(into: &mut Dictionary, stronger: &Dictionary) {
    for (key, value) in stronger {
        into.insert(key.clone(), value.clone());
    }
}

/// Whether a session layer's own rate governs the stack: it authors
/// timeCodesPerSecond, or the root does not and the session authors
/// framesPerSecond.
fn session_governs(layers: &Layers, session: usize, root: usize) -> bool {
    layers
        .field(session, field::TIME_CODES_PER_SECOND)
        .is_some()
        || (layers.field(root, field::TIME_CODES_PER_SECOND).is_none()
            && layers.field(session, field::FRAMES_PER_SECOND).is_some())
}

#[allow(clippy::too_many_arguments)]
fn build(
    layers: &mut Layers,
    layer: usize,
    offset: LayerOffset,
    rate: f64,
    session_owner: Option<&Token>,
    stack: &mut LayerStack,
    seen: &mut Vec<usize>,
    problems: &mut Problems,
) -> Result<()> {
    if seen.len() > MOST_SUBLAYER_DEPTH {
        return Err(Error::limit("sublayers nested more than 256 deep"));
    }
    seen.push(layer);
    stack.layers.push(layer);
    stack.offsets.push(offset);
    let paths: Vec<Token> = layers
        .field(layer, field::SUB_LAYERS)
        .and_then(Value::as_tokens)
        .unwrap_or(&[])
        .to_vec();
    let offsets: Vec<LayerOffset> = match layers.field(layer, field::SUB_LAYER_OFFSETS) {
        Some(Value::LayerOffsetVector(offsets)) => offsets.to_vec(),
        _ => Vec::new(),
    };
    let mut sublayers = Vec::new();
    for (at, path) in paths.iter().enumerate() {
        let mut asset = path.clone();
        if is_expression(&asset) {
            // Expressions evaluating to nothing (or failing) are skipped.
            match evaluate_to_string(&asset, &stack.variables) {
                Some(text) => asset = text,
                None => continue,
            }
        }
        if asset.is_empty() {
            continue;
        }
        let identifier = assets::anchored(&asset, layers.identifier(layer));
        let Some(sublayer) = layers.find_or_open(&identifier)? else {
            problems.add(format!("Could not load sublayer @{identifier}@"));
            continue;
        };
        if seen.contains(&sublayer) {
            problems.add(format!("Sublayer cycle through @{identifier}@"));
            continue;
        }
        let mut own = offsets.get(at).copied().unwrap_or(LayerOffset::IDENTITY);
        if !valid_offset(&own) {
            own = LayerOffset::IDENTITY;
        }
        let sub_rate = layers.time_codes_per_second(sublayer);
        if rate != sub_rate {
            own.scale = own.scale * rate / sub_rate;
        }
        sublayers.push((sublayer, offset.concatenate(&own), sub_rate));
    }
    // The session owner's sublayers first, when the layer has owned
    // sublayers (_ApplyOwnedSublayerOrder, a stable partition).
    if let Some(owner) = session_owner
        && layers
            .field(layer, field::HAS_OWNED_SUB_LAYERS)
            .and_then(Value::as_bool)
            == Some(true)
    {
        let (mine, rest): (Vec<_>, Vec<_>) = sublayers.into_iter().partition(|(sublayer, _, _)| {
            layers.token_field(*sublayer, "owner").as_ref() == Some(owner)
        });
        sublayers = mine.into_iter().chain(rest).collect();
    }
    for (sublayer, sub_offset, sub_rate) in sublayers {
        build(
            layers,
            sublayer,
            sub_offset,
            sub_rate,
            session_owner,
            stack,
            seen,
            problems,
        )?;
    }
    seen.pop();
    Ok(())
}

/// Finite, with a positive scale.
pub fn valid_offset(offset: &LayerOffset) -> bool {
    offset.offset.is_finite() && offset.scale.is_finite() && offset.scale > 0.0
}
