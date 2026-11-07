//! Authoring specs into a layer the way Sdf's own authoring does: prims
//! with their specifier and type, attributes with their type name and
//! variability, relationships with explicit targets, and the children and
//! property lists that order them.

use std::sync::Arc;

use crate::usd::sdf::{
    LayerData, ListOp, Path, SpecType, Specifier, Token, Value, Variability, field,
};

/// Builds a layer spec by spec.
pub struct Author {
    layer: LayerData,
}

impl Default for Author {
    fn default() -> Author {
        Author::new()
    }
}

impl Author {
    /// An empty layer.
    pub fn new() -> Author {
        Author {
            layer: LayerData::new(),
        }
    }

    /// The layer built.
    pub fn finish(self) -> LayerData {
        self.layer
    }

    /// Whether a spec is at `path`.
    pub fn has(&self, path: &Path) -> bool {
        self.layer.has_spec(path)
    }

    /// Set a layer metadata field.
    pub fn layer_metadata(&mut self, name: &str, value: Value) {
        self.layer.root_mut().set(Token::new(name), value);
    }

    fn append_token(&mut self, owner: &Path, list: &str, name: Token) {
        let spec = self.layer.spec_mut(owner).expect("an owner spec");
        let mut names = spec.tokens(list).to_vec();
        if !names.contains(&name) {
            names.push(name);
            spec.set(Token::new(list), Value::token_vector(names));
        }
    }

    /// `def <type_name> "<name>"` at `path`, with its ancestors made as
    /// `def Xform` prims when missing. Returns whether it was new.
    pub fn prim(&mut self, path: &Path, type_name: &str) -> bool {
        if self.layer.has_spec(path) {
            return false;
        }
        let parent = path.parent();
        if !parent.is_absolute_root() && !self.layer.has_spec(&parent) {
            self.prim(&parent, "Xform");
        }
        let spec = self.layer.create_spec(path.clone(), SpecType::Prim);
        spec.set(field::SPECIFIER, Value::Specifier(Specifier::Def));
        if !type_name.is_empty() {
            spec.set(field::TYPE_NAME, Value::Token(Token::new(type_name)));
        }
        self.append_token(&parent, field::PRIM_CHILDREN, path.name_token());
        true
    }

    /// Set a prim metadata field.
    pub fn prim_metadata(&mut self, path: &Path, name: &str, value: Value) {
        if let Some(spec) = self.layer.spec_mut(path) {
            spec.set(Token::new(name), value);
        }
    }

    /// An attribute with a default value; `metadata` adds fields
    /// (`interpolation`, `elementSize`, ...).
    pub fn attribute(
        &mut self,
        prim: &Path,
        name: &str,
        type_name: &str,
        value: Value,
        uniform: bool,
    ) -> Path {
        let path = prim.property(name);
        if !self.layer.has_spec(&path) {
            self.layer.create_spec(path.clone(), SpecType::Attribute);
            self.append_token(prim, field::PROPERTIES, Token::new(name));
        }
        let spec = self.layer.spec_mut(&path).expect("the attribute");
        spec.set(field::CUSTOM, Value::Bool(false));
        spec.set(field::TYPE_NAME, Value::Token(Token::new(type_name)));
        spec.set(
            field::VARIABILITY,
            Value::Variability(if uniform {
                Variability::Uniform
            } else {
                Variability::Varying
            }),
        );
        spec.set(field::DEFAULT, value);
        path
    }

    /// An attribute declared without a value.
    pub fn declare(&mut self, prim: &Path, name: &str, type_name: &str) -> Path {
        let path = self.attribute(prim, name, type_name, Value::ValueBlock, false);
        if let Some(spec) = self.layer.spec_mut(&path) {
            spec.erase(field::DEFAULT);
        }
        path
    }

    /// Set a metadata field on a property.
    pub fn property_metadata(&mut self, property: &Path, name: &str, value: Value) {
        if let Some(spec) = self.layer.spec_mut(property) {
            spec.set(Token::new(name), value);
        }
    }

    /// Connect the attribute at `attribute` (made first) to `sources`
    /// (`<name>.connect = [<sources>]`).
    pub fn connect(&mut self, attribute: &Path, sources: Vec<Path>) {
        for source in &sources {
            self.layer
                .create_spec(attribute.target(source.clone()), SpecType::Connection);
        }
        let Some(spec) = self.layer.spec_mut(attribute) else {
            return;
        };
        spec.set(
            field::CONNECTION_CHILDREN,
            Value::PathVector(Arc::new(sources.clone())),
        );
        spec.set(
            field::CONNECTION_PATHS,
            Value::PathListOp(Arc::new(ListOp::explicit(sources))),
        );
    }

    /// A relationship with explicit targets.
    pub fn relationship(&mut self, prim: &Path, name: &str, targets: Vec<Path>) {
        let path = prim.property(name);
        if !self.layer.has_spec(&path) {
            self.layer.create_spec(path.clone(), SpecType::Relationship);
            self.append_token(prim, field::PROPERTIES, Token::new(name));
        }
        for target in &targets {
            self.layer
                .create_spec(path.target(target.clone()), SpecType::RelationshipTarget);
        }
        let spec = self.layer.spec_mut(&path).expect("the relationship");
        spec.set(field::CUSTOM, Value::Bool(false));
        spec.set(field::VARIABILITY, Value::Variability(Variability::Uniform));
        spec.set(
            field::TARGET_CHILDREN,
            Value::PathVector(Arc::new(targets.clone())),
        );
        spec.set(
            field::TARGET_PATHS,
            Value::PathListOp(Arc::new(ListOp::explicit(targets))),
        );
    }

    /// Prepend API schemas to a prim's `apiSchemas`.
    pub fn api_schema(&mut self, prim: &Path, schema: &str) {
        let Some(spec) = self.layer.spec_mut(prim) else {
            return;
        };
        let mut list = match spec.get("apiSchemas") {
            Some(Value::TokenListOp(list)) => (**list).clone(),
            _ => ListOp::default(),
        };
        let token = Token::new(schema);
        if !list.prepended.contains(&token) {
            list.prepended.push(token);
        }
        spec.set("apiSchemas", Value::TokenListOp(Arc::new(list)));
    }
}
