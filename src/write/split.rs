//! Polygons split into prims by their `name` path: `/Lens/Ring 2` a Mesh prim
//! `Ring_2` under an Xform `Lens`, beneath the item's own prim, so what
//! Import Object merged goes out as the hierarchy it came from. Each prim
//! holds its faces' points, attributes and material subsets; the placements
//! stay in the points.
use super::{Out, Shaping, geometry};
use crate::names;
use crate::usd::scene::Node;
use kinogaki_studio_api::Error;
use kinogaki_studio_api::edit::editor::extract;
use kinogaki_studio_api::format::Unique;
use kinogaki_studio_api::geometry::{Domain, Geometry, names as attribute};
use std::collections::HashMap;

/// One step of the paths.
#[derive(Default)]
struct Step {
    name: String,
    children: Vec<usize>,
    /// Polygons named exactly this path.
    polygons: Vec<u32>,
}

/// `value`'s polygons under their `name` paths (each starting at `/`: other
/// names are words, an OBJ piece's); `None` when they name fewer than two
/// different things or anything isn't a polygon (curves stay with the
/// value's own prims).
fn tree(value: &Geometry) -> Option<Vec<Step>> {
    let names = value
        .attributes()
        .text(Domain::Primitive, attribute::NAME)?;
    let table = value.primitives();
    if !(0..table.len()).all(|p| table.kind(p).is_polygon()) {
        return None;
    }
    let mut steps = vec![Step::default()];
    let mut at: HashMap<(usize, &str), usize> = HashMap::new();
    let mut leaves = std::collections::HashSet::new();
    for p in 0..table.len() {
        let path = names.get(p).filter(|n| n.starts_with('/'))?;
        leaves.insert(path);
        let mut here = 0;
        for part in path.split('/').filter(|s| !s.is_empty()) {
            here = *at.entry((here, part)).or_insert_with(|| {
                steps.push(Step {
                    name: part.to_owned(),
                    ..Step::default()
                });
                let new = steps.len() - 1;
                steps[here].children.push(new);
                new
            });
        }
        steps[here].polygons.push(p as u32);
    }
    (leaves.len() > 1).then_some(steps)
}

impl Out<'_> {
    /// `value`'s polygons as prims under the node at `parent`, appended to
    /// `children` (whose first would be node `first`), when its `name`s
    /// split it; whether they did.
    pub(super) fn split_by_name(
        &mut self,
        value: &Geometry,
        (parent, first): (usize, usize),
        names_beside: &mut Unique,
        children: &mut Vec<Node>,
    ) -> Result<bool, Error> {
        let Some(steps) = tree(value) else {
            return Ok(false);
        };
        let mut stack: Vec<(usize, usize, Option<usize>)> = (steps[0].children.iter().rev())
            .map(|&s| (s, parent, None))
            .collect();
        let mut beside: HashMap<usize, Unique> = HashMap::new();
        while let Some((step, under, _)) = stack.pop() {
            let s = &steps[step];
            let wanted = names::identifier(&s.name);
            let name = match under == parent {
                true => names_beside.take(&wanted),
                false => beside.entry(under).or_default().take(&wanted),
            };
            let at = first + children.len();
            children.push(Node::group(&name, Some(under)));
            if !s.polygons.is_empty() {
                let cut = self.part_of(value, &s.polygons)?;
                let prims = self.prims_of(&cut);
                match (prims.len(), s.children.is_empty()) {
                    (1, true) => {
                        let (_, kind, material) = prims.into_iter().next().expect("one");
                        let node = children.last_mut().expect("just pushed");
                        (node.kind, node.material) = (kind, material);
                    }
                    _ => {
                        for (suffix, kind, material) in prims {
                            let held = if suffix.is_empty() {
                                "Geometry"
                            } else {
                                suffix
                            };
                            let inside = beside.entry(at).or_default().take(held);
                            children.push(Node {
                                kind,
                                material,
                                ..Node::group(&inside, Some(at))
                            });
                        }
                    }
                }
            }
            stack.extend(s.children.iter().rev().map(|&c| (c, at, None)));
        }
        Ok(true)
    }

    /// `polygons` of `value` as a value of their own, knowing nothing of
    /// the paths they came by.
    fn part_of(&self, value: &Geometry, polygons: &[u32]) -> Result<Geometry, Error> {
        let cut = extract(value, polygons, &[])?;
        let attributes = cut
            .attributes()
            .without(Domain::Primitive, attribute::NAME)
            .without(Domain::Primitive, attribute::PART);
        cut.with_attributes(attributes)
    }

    fn prims_of(
        &mut self,
        value: &Geometry,
    ) -> Vec<(&'static str, crate::usd::scene::NodeKind, Option<usize>)> {
        let shaping = Shaping {
            materials: &self.materials,
            double_sided: &self.double_sided,
            colors: self.writing.colors,
        };
        geometry::prims(value, &shaping, &mut self.warnings).prims
    }
}
