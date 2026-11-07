// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Properties flattened (`_CopyProperty`): an attribute keeps its composed
//! type, custom, variability and metadata, the time samples of its strongest
//! value opinion when that opinion is samples (moved into stage time), its
//! strongest default when any opinion authors one, and its connections in
//! stage namespace as an explicit list; a relationship keeps its metadata and
//! its targets likewise. Built-in properties of the prim's schemas take their
//! type and variability from the schema and are not custom.

use std::sync::Arc;

use super::metadata::private_field;
use super::{Flattener, Opinion};
use crate::usd::compose::graph::PrimIndex;
use crate::usd::error::Result;
use crate::usd::sdf::{ListOp, Path, SpecType, Token, Value, Variability, field};

impl Flattener<'_> {
    /// Flatten property `name` of the prim at `path`.
    pub(super) fn property(&mut self, index: &PrimIndex, path: &Path, name: &Token) -> Result<()> {
        let opinions = self.gather(index, Some(name));
        let Some(first) = opinions.first() else {
            return Ok(());
        };
        let kind = self.opinion_kind(first);
        // Specs of the other property kind are not part of this property.
        let kept: Vec<Opinion> = opinions
            .iter()
            .filter(|opinion| self.opinion_kind(opinion) == kind)
            .cloned()
            .collect();
        let attribute = kind == SpecType::Attribute;
        let builtin = self
            .builtin(name)
            .filter(|found| found.relationship != attribute);
        if attribute && builtin.is_none() && self.type_name(&kept).is_empty() {
            return Ok(());
        }
        let property_path = path.property(name.clone());
        self.out.create_spec(property_path.clone(), kind);
        self.properties
            .entry(path.clone())
            .or_default()
            .push(name.clone());
        let mut fields: Vec<Token> = Vec::new();
        for opinion in &kept {
            let Some(spec) = self
                .composer
                .layers
                .layer(opinion.layer)
                .spec(&opinion.path)
            else {
                continue;
            };
            for (field_name, _) in &spec.fields {
                if !private_field(field_name, kind) && !fields.contains(field_name) {
                    fields.push(field_name.clone());
                }
            }
        }
        let mut required = vec![Token::new(field::CUSTOM), Token::new(field::VARIABILITY)];
        if attribute && builtin.is_some() {
            required.push(Token::new(field::TYPE_NAME));
        }
        for key in required {
            if !fields.contains(&key) {
                fields.push(key);
            }
        }
        for key in fields {
            let value = match key.as_str() {
                // Custom and variability come from the weakest spec of any
                // kind: both are required fields, so a spec without one gives
                // the fallback.
                field::CUSTOM => {
                    let custom = self
                        .weakest_spec_field(&opinions, field::CUSTOM)
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some(Value::Bool(custom && builtin.is_none()))
                }
                field::VARIABILITY if attribute => {
                    let variability = match &builtin {
                        Some(found) if found.uniform => Variability::Uniform,
                        Some(_) => Variability::Varying,
                        None => match self.weakest_spec_field(&opinions, field::VARIABILITY) {
                            Some(Value::Variability(found)) => *found,
                            _ => Variability::Varying,
                        },
                    };
                    Some(Value::Variability(variability))
                }
                field::TYPE_NAME if attribute && builtin.is_some() => builtin
                    .as_ref()
                    .map(|found| Value::Token(found.type_name.clone())),
                _ => self.composed_value(index, &kept, &key),
            };
            if let Some(value) = value {
                self.out.set(&property_path, &key, value);
            }
        }
        let targets = self.composer.target_paths(index, name, attribute)?;
        if !targets.is_empty() {
            let items = targets
                .iter()
                .map(|target| self.in_prototype(target))
                .collect();
            let key = if attribute {
                field::CONNECTION_PATHS
            } else {
                field::TARGET_PATHS
            };
            self.out.set(
                &property_path,
                key,
                Value::PathListOp(Arc::new(ListOp::explicit(items))),
            );
        }
        if attribute {
            self.attribute_values(index, &kept, &property_path);
        }
        Ok(())
    }

    /// An attribute's samples (when its strongest value opinion is samples)
    /// and default (when any opinion has one).
    fn attribute_values(&mut self, index: &PrimIndex, opinions: &[Opinion], path: &Path) {
        for opinion in opinions {
            if let Some(found) = self.opinion_field(opinion, field::TIME_SAMPLES) {
                let Value::TimeSamples(samples) = found else {
                    break;
                };
                if samples.is_empty() {
                    continue;
                }
                let offset = self.layer_offset(index, opinion);
                let moved = samples
                    .iter()
                    .map(|(time, value)| {
                        (offset.apply(*time), self.to_stage(index, opinion, value))
                    })
                    .collect::<Vec<_>>();
                self.out.set(
                    path,
                    field::TIME_SAMPLES,
                    Value::TimeSamples(Arc::new(moved)),
                );
                break;
            }
            if self.opinion_field(opinion, field::DEFAULT).is_some() {
                break;
            }
        }
        // An animation block is no default: a weaker one shows through, and
        // with none the default is blocked.
        let mut blocked = false;
        for opinion in opinions {
            let Some(found) = self.opinion_field(opinion, field::DEFAULT) else {
                continue;
            };
            if matches!(found, Value::AnimationBlock) {
                blocked = true;
                continue;
            }
            let value = self.to_stage(index, opinion, found);
            self.out.set(path, field::DEFAULT, value);
            blocked = false;
            break;
        }
        if blocked {
            self.out.set(path, field::DEFAULT, Value::ValueBlock);
        }
    }
}
