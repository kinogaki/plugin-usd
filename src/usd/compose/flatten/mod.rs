// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! A composed stage flattened into one layer, after `UsdStage::Flatten`
//! (`pxr/usd/usd/stage.cpp`: `_CopyPrim`, `_CopyProperty`, `_CopyMetadata`):
//! the stage metadata of the session and root layers, then every active
//! prim depth first with its composed metadata and properties. Opinions are
//! read as `Usd_Resolver` walks them: nodes in strength order, each node's
//! layers strongest first; values move into stage time by their layer's
//! offset, and asset paths are anchored at the layer that authored them.
//!
//! - `mod.rs`: the walk over prims, and writing specs
//! - `instances`: instances sharing prototypes
//! - `metadata`: composed metadata fields
//! - `properties`: attributes and relationships
//! - `schemas`, `schema_table`: built-in properties of prim types

mod instances;
mod metadata;
mod properties;
mod schema_table;
mod schemas;

use super::composer::Composer;
use super::graph::PrimIndex;
use crate::usd::error::Result;
use crate::usd::sdf::identifier::dictionary_order;
use crate::usd::sdf::{LayerData, Path, PathMap, SpecType, Token, Value, field};

/// How a stage is flattened.
#[derive(Clone, Copy, Debug)]
pub struct FlattenOptions {
    /// Add OpenUSD's "Generated from Composed Stage" note to the layer's
    /// documentation.
    pub comment: bool,
    /// Where instances' prims go.
    pub instances: Instances,
}

/// Where a flattened stage's instances find their prims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instances {
    /// Each instance holds its own copy of its prototype's prims, as
    /// instance proxies show them.
    Copied,
    /// Instances reference shared prototypes, root `over`s named
    /// `Flattened_Prototype_N` (past the names the stage's root prims
    /// take), as `UsdStage::Flatten` writes them.
    Referenced,
    /// A prototype's prims are written once, beneath its source instance
    /// (its shallowest, then first in path order); the other instances hold
    /// none. Every prim is at a path of the stage.
    AtSource,
}

/// A flattened stage: its layer, and the prototype each instance shows.
#[derive(Clone, Debug)]
pub struct Flattened {
    /// The layer.
    pub layer: LayerData,
    /// Each instance's prototype, by the prim its prims are the children
    /// of in the layer: a `Flattened_Prototype_N` ([`Instances::Referenced`])
    /// or its source instance ([`Instances::AtSource`]); none when instances
    /// are [`Copied`](Instances::Copied).
    pub instances: PathMap<Path>,
}

/// One place an opinion can be: a node's layer and the spec path in it.
#[derive(Clone)]
pub(super) struct Opinion {
    node: u32,
    layer: usize,
    position: usize,
    path: Path,
}

/// One prototype: its instances' key, its source instance, and its path in
/// the flattened layer.
struct Prototype {
    key: super::instancing::InstanceKey,
    source: Path,
    flattened: Path,
    /// Its `__Prototype_N` number (0 until numbered).
    number: usize,
}

/// The flattening's state.
pub(super) struct Flattener<'a> {
    composer: &'a mut Composer,
    out: LayerData,
    /// Child and property names per output prim (and the pseudo-root).
    children: PathMap<Vec<Token>>,
    properties: PathMap<Vec<Token>>,
    prototypes: Vec<Prototype>,
    /// Each instance's prototype.
    instance_of: PathMap<usize>,
    /// While a prototype is written: its source and flattened paths.
    writing: Option<(Path, Path)>,
    instances: Instances,
    /// The prim being written's type and applied API schemas.
    prim_type: Token,
    prim_applied: Vec<Token>,
}

/// Flatten the composer's stage into a new layer.
pub fn flatten_stage(composer: &mut Composer, options: FlattenOptions) -> Result<Flattened> {
    let mut flat = Flattener {
        composer,
        out: LayerData::new(),
        children: PathMap::default(),
        properties: PathMap::default(),
        prototypes: Vec::new(),
        instance_of: PathMap::default(),
        writing: None,
        instances: options.instances,
        prim_type: Token::empty(),
        prim_applied: Vec::new(),
    };
    flat.pseudo_root(options.comment)?;
    let root = Path::absolute_root();
    let root_index = flat.composer.index(&root)?;
    let roots: Vec<Path> = flat
        .composer
        .child_names(&root_index)
        .into_iter()
        .map(|name| root.child(name))
        .collect();
    match options.instances {
        Instances::Copied => {}
        Instances::Referenced => {
            flat.populate(&roots)?;
            flat.write_prototypes(&roots)?;
        }
        Instances::AtSource => {
            flat.populate(&roots)?;
            for prototype in &mut flat.prototypes {
                prototype.flattened = prototype.source.clone();
            }
        }
    }
    for path in &roots {
        flat.prims(path, path)?;
    }
    flat.finish();
    let instances = (flat.instance_of.iter())
        .map(|(instance, &p)| (instance.clone(), flat.prototypes[p].flattened.clone()))
        .collect();
    Ok(Flattened {
        layer: flat.out,
        instances,
    })
}

impl Flattener<'_> {
    /// Write the children and properties fields.
    fn finish(&mut self) {
        for (path, names) in std::mem::take(&mut self.children) {
            if !names.is_empty() {
                self.out
                    .set(&path, field::PRIM_CHILDREN, Value::token_vector(names));
            }
        }
        for (path, names) in std::mem::take(&mut self.properties) {
            if !names.is_empty() {
                self.out
                    .set(&path, field::PROPERTIES, Value::token_vector(names));
            }
        }
    }

    /// Stage metadata: the session layer's fields, then the root layer's
    /// (not their sublayers'), composed (`_ComposePseudoRootMetadataImpl`).
    fn pseudo_root(&mut self, comment: bool) -> Result<()> {
        let root = Path::absolute_root();
        let index = self.composer.index(&root)?;
        let stack = self.composer.stacks.stack(self.composer.root_stack);
        let mut opinions = Vec::new();
        if let Some(session) = stack.session {
            opinions.push(Opinion {
                node: 0,
                layer: session,
                position: 0,
                path: root.clone(),
            });
        }
        if let Some(position) = stack.layers.iter().position(|&layer| layer == stack.root) {
            opinions.push(Opinion {
                node: 0,
                layer: stack.root,
                position,
                path: root.clone(),
            });
        }
        let mut fields: Vec<Token> = Vec::new();
        for opinion in &opinions {
            for (name, _) in &self.composer.layers.layer(opinion.layer).root().fields {
                if !metadata::private_field(name, SpecType::PseudoRoot) && !fields.contains(name) {
                    fields.push(name.clone());
                }
            }
        }
        for key in fields {
            if let Some(value) = self.composed_value(&index, &opinions, &key) {
                self.out.root_mut().set(key, value);
            }
        }
        if comment {
            let identifier = self.composer.layers.identifier(stack.root).to_owned();
            self.add_comment(&identifier);
        }
        Ok(())
    }

    /// Append "Generated from Composed Stage of root layer <path>" to the
    /// flattened layer's documentation.
    fn add_comment(&mut self, root: &str) {
        let mut text = String::new();
        if let Some(held) = self
            .out
            .root()
            .get(field::DOCUMENTATION)
            .and_then(Value::as_token)
            && !held.is_empty()
        {
            text.push_str(held);
            text.push_str("\n\n");
        }
        text.push_str("Generated from Composed Stage of root layer ");
        text.push_str(root);
        text.push('\n');
        self.out
            .root_mut()
            .set(field::DOCUMENTATION, Value::String(Token::from(text)));
    }

    /// Flatten the prim whose index is at `source` into `path`, and its
    /// descendants unless it is an instance (`_CopyPrim` over the range),
    /// depth first without recursion.
    fn prims(&mut self, source: &Path, path: &Path) -> Result<()> {
        let mut work = vec![(source.clone(), path.clone())];
        while let Some((source, path)) = work.pop() {
            self.composer.check()?;
            let children = self.prim(&source, &path)?;
            for name in children.into_iter().rev() {
                work.push((source.child(name.clone()), path.child(name)));
            }
        }
        Ok(())
    }

    /// Flatten one prim; its children's names to flatten next.
    fn prim(&mut self, source: &Path, path: &Path) -> Result<Vec<Token>> {
        let index = self.composer.index(source)?;
        let opinions = self.gather(&index, None);
        if !self.is_active(&opinions) {
            return Ok(Vec::new());
        }
        self.out.create_spec(path.clone(), SpecType::Prim);
        self.children
            .entry(path.parent())
            .or_default()
            .push(path.name_token());
        let instance = index.instanceable && self.instances != Instances::Copied;
        if instance && self.instances == Instances::Referenced {
            self.reference_prototype(source, path);
        }
        self.prim_metadata(&index, &opinions, path);
        self.definition_of(&index, &opinions);
        // Properties by name (TfDictionaryLessThan), then propertyOrder.
        let mut names = self.composer.property_names(&index);
        names.sort_by(|a, b| dictionary_order(a, b));
        if let Some(Value::TokenVector(order)) =
            self.composed_value(&index, &opinions, field::PROPERTY_ORDER)
        {
            apply_property_order(&mut names, &order);
        }
        for name in names {
            self.property(&index, path, &name)?;
        }
        // An instance's prims are its prototype's: beneath its source alone,
        // or in the prototype written apart.
        if instance && !self.holds_prototype(source) {
            return Ok(Vec::new());
        }
        Ok(self.composer.child_names(&index))
    }

    /// Whether the instance at `source` holds its prototype's prims: it is
    /// the source the prototype is written beneath ([`Instances::AtSource`]).
    fn holds_prototype(&self, source: &Path) -> bool {
        self.instances == Instances::AtSource
            && (self.instance_of.get(source)).is_some_and(|&p| self.prototypes[p].source == *source)
    }

    /// The opinions of the prim (`property` none) or of one of its
    /// properties.
    fn gather(&self, index: &PrimIndex, property: Option<&Token>) -> Vec<Opinion> {
        let mut out = Vec::new();
        for node in index.order(true) {
            let entry = index.node(node);
            if !entry.contributes() {
                continue;
            }
            let path = match property {
                Some(name) => entry.path.property(name.clone()),
                None => entry.path.clone(),
            };
            for (position, &layer) in self
                .composer
                .stacks
                .stack(entry.stack)
                .layers
                .iter()
                .enumerate()
            {
                if self.composer.layers.has_spec(layer, &path) {
                    out.push(Opinion {
                        node,
                        layer,
                        position,
                        path: path.clone(),
                    });
                }
            }
        }
        out
    }

    /// The prim's typed schema and applied API schemas, for its properties.
    fn definition_of(&mut self, index: &PrimIndex, opinions: &[Opinion]) {
        self.prim_type = self.type_name(opinions);
        self.prim_applied = match self.composed_value(index, opinions, "apiSchemas") {
            Some(Value::TokenListOp(list)) => list.explicit_items.clone(),
            _ => Vec::new(),
        };
    }

    /// The built-in definition of property `name` of the prim being
    /// written, if any.
    fn builtin(&self, name: &str) -> Option<schemas::PropertyDefinition> {
        schemas::find(&self.prim_type, &self.prim_applied, name)
    }
}

/// `UsdPrim`'s `ApplyPropertyOrder`: the names `order` lists come first in
/// its order; the rest follow in their order.
fn apply_property_order(names: &mut Vec<Token>, order: &[Token]) {
    if order.is_empty() {
        return;
    }
    let mut first: Vec<Token> = Vec::new();
    for name in order {
        if names.contains(name) && !first.contains(name) {
            first.push(name.clone());
        }
    }
    let rest: Vec<Token> = names
        .iter()
        .filter(|name| !first.contains(name))
        .cloned()
        .collect();
    *names = first;
    names.extend(rest);
}
