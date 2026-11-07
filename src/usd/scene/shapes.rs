//! UsdGeom's intrinsic shapes (Cube, Sphere, Cylinder, Cone, Capsule,
//! Plane) as authored, and tessellated into polygon meshes on request: 24
//! segments around and 12 along, quads with triangle fans at poles and
//! caps, faces wound counterclockwise seen from outside.

use std::f64::consts::TAU;
use std::sync::Arc;

use super::mesh::{Mesh, Subdivision};
use super::primvar::{Interpolation, Primvar, read_primvars};
use super::values::{Time, token_at, value_at};
use crate::usd::sdf::{LayerData, Path, Token};

/// Segments around a round shape.
const AROUND: usize = 24;
/// Segments from pole to pole of a sphere (and per capsule hemisphere / 2).
const ALONG: usize = 12;

/// Which shape, with its schema's attributes (`size`, `radius`, `height`,
/// `width`, `length`; `axis` is `X`, `Y` or `Z`).
#[allow(missing_docs)]
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeForm {
    /// `Cube`: edge `size`.
    Cube { size: f64 },
    /// `Sphere`.
    Sphere { radius: f64 },
    /// `Cylinder` along `axis`.
    Cylinder {
        radius: f64,
        height: f64,
        axis: Token,
    },
    /// `Cone` along `axis`, its apex at +height/2.
    Cone {
        radius: f64,
        height: f64,
        axis: Token,
    },
    /// `Capsule`: a cylinder of `height` with hemispheres at its ends.
    Capsule {
        radius: f64,
        height: f64,
        axis: Token,
    },
    /// `Plane` of `width` and `length`, facing `axis`.
    Plane {
        width: f64,
        length: f64,
        axis: Token,
    },
}

/// One intrinsic shape prim.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// The shape and its sizes.
    pub form: ShapeForm,
    /// Its constant primvars (`displayColor`, …) in name order.
    pub primvars: Vec<Primvar>,
    /// `doubleSided`.
    pub double_sided: bool,
}

impl Shape {
    /// The shape as a polygon mesh, its primvars constant over it.
    pub fn mesh(&self) -> Mesh {
        let t = match &self.form {
            ShapeForm::Cube { size } => cube(*size),
            ShapeForm::Sphere { radius } => sphere(*radius),
            ShapeForm::Cylinder {
                radius,
                height,
                axis,
            } => cylinder(*radius, *height, axis),
            ShapeForm::Cone {
                radius,
                height,
                axis,
            } => cone(*radius, *height, axis),
            ShapeForm::Capsule {
                radius,
                height,
                axis,
            } => capsule(*radius, *height, axis),
            ShapeForm::Plane {
                width,
                length,
                axis,
            } => plane(*width, *length, axis),
        };
        Mesh {
            points: Arc::new(t.points.iter().map(|p| p.map(|c| c as f32)).collect()),
            face_vertex_counts: Arc::new(t.counts.iter().map(|&c| c as i32).collect()),
            face_vertex_indices: Arc::new(t.indices.iter().map(|&i| i as i32).collect()),
            left_handed: false,
            primvars: self.primvars.clone(),
            subdivision: Subdivision::default(),
            subsets: Vec::new(),
            double_sided: self.double_sided,
        }
    }

    /// The schema type name.
    pub fn type_name(&self) -> &'static str {
        match self.form {
            ShapeForm::Cube { .. } => "Cube",
            ShapeForm::Sphere { .. } => "Sphere",
            ShapeForm::Cylinder { .. } => "Cylinder",
            ShapeForm::Cone { .. } => "Cone",
            ShapeForm::Capsule { .. } => "Capsule",
            ShapeForm::Plane { .. } => "Plane",
        }
    }
}

/// Read the shape of type `kind` at `prim`, with each schema's fallbacks;
/// none for another type.
pub(super) fn read(layer: &LayerData, prim: &Path, kind: &str, time: Time) -> Option<Shape> {
    let number = |name: &str, fallback: f64| {
        (value_at(layer, prim, name, time))
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
            .unwrap_or(fallback)
    };
    let axis = || token_at(layer, prim, "axis", "Z");
    let form = match kind {
        "Cube" => ShapeForm::Cube {
            size: number("size", 2.0),
        },
        "Sphere" => ShapeForm::Sphere {
            radius: number("radius", 1.0),
        },
        "Cylinder" => ShapeForm::Cylinder {
            radius: number("radius", 1.0),
            height: number("height", 2.0),
            axis: axis(),
        },
        "Cone" => ShapeForm::Cone {
            radius: number("radius", 1.0),
            height: number("height", 2.0),
            axis: axis(),
        },
        "Capsule" => ShapeForm::Capsule {
            radius: number("radius", 0.5),
            height: number("height", 1.0),
            axis: axis(),
        },
        "Plane" => ShapeForm::Plane {
            width: number("width", 2.0),
            length: number("length", 2.0),
            axis: axis(),
        },
        _ => return None,
    };
    let mut primvars = read_primvars(layer, prim, &[], time);
    primvars.retain(|p| p.interpolation == Interpolation::Constant && p.fits(1));
    Some(Shape {
        form,
        primvars,
        double_sided: (value_at(layer, prim, "doubleSided", Time::Default))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// A tessellated shape: points, face counts and face indices.
struct Tessellation {
    /// Points.
    pub points: Vec<[f64; 3]>,
    /// Vertices per face.
    pub counts: Vec<u32>,
    /// Point indices per vertex.
    pub indices: Vec<u32>,
}

impl Tessellation {
    fn new() -> Tessellation {
        Tessellation {
            points: Vec::new(),
            counts: Vec::new(),
            indices: Vec::new(),
        }
    }

    fn face(&mut self, ids: &[u32]) {
        self.counts.push(ids.len() as u32);
        self.indices.extend_from_slice(ids);
    }

    /// Rotate from the Z-axis frame shapes are built in to `axis`.
    fn orient(mut self, axis: &str) -> Tessellation {
        for p in &mut self.points {
            *p = match axis {
                "X" => [p[2], p[0], p[1]],
                "Y" => [p[1], p[2], p[0]],
                _ => *p,
            };
        }
        self
    }
}

/// A box of edge `size` centered at the origin.
fn cube(size: f64) -> Tessellation {
    let h = size / 2.0;
    let mut t = Tessellation::new();
    for z in [-h, h] {
        for y in [-h, h] {
            for x in [-h, h] {
                t.points.push([x, y, z]);
            }
        }
    }
    // Corner (x, y, z) bits: x 1, y 2, z 4.
    for face in [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ] {
        t.face(&face);
    }
    t
}

/// Rings of a surface of revolution about Z: (radius, z) per ring, closed
/// at an end by a pole point when its radius is 0, else by a cap face.
fn revolve(rings: &[(f64, f64)]) -> Tessellation {
    let mut t = Tessellation::new();
    let mut starts = Vec::new();
    for &(radius, z) in rings {
        starts.push(t.points.len() as u32);
        if radius == 0.0 {
            t.points.push([0.0, 0.0, z]);
            continue;
        }
        for step in 0..AROUND {
            let angle = TAU * step as f64 / AROUND as f64;
            t.points
                .push([radius * angle.cos(), radius * angle.sin(), z]);
        }
    }
    let at = |ring: usize, step: usize| starts[ring] + (step % AROUND) as u32;
    for ring in 0..rings.len() - 1 {
        let (below, above) = (rings[ring].0 == 0.0, rings[ring + 1].0 == 0.0);
        for step in 0..AROUND {
            match (below, above) {
                (true, true) => {}
                (true, false) => {
                    t.face(&[starts[ring], at(ring + 1, step), at(ring + 1, step + 1)])
                }
                (false, true) => t.face(&[at(ring, step), at(ring, step + 1), starts[ring + 1]]),
                (false, false) => t.face(&[
                    at(ring, step),
                    at(ring, step + 1),
                    at(ring + 1, step + 1),
                    at(ring + 1, step),
                ]),
            }
        }
    }
    // Flat caps where an end ring is not a pole.
    if rings[0].0 != 0.0 {
        let cap: Vec<u32> = (0..AROUND).rev().map(|step| at(0, step)).collect();
        t.face(&cap);
    }
    let last = rings.len() - 1;
    if rings[last].0 != 0.0 {
        let cap: Vec<u32> = (0..AROUND).map(|step| at(last, step)).collect();
        t.face(&cap);
    }
    t
}

/// A sphere of `radius`.
fn sphere(radius: f64) -> Tessellation {
    let rings: Vec<(f64, f64)> = (0..=ALONG)
        .map(|ring| {
            let polar = std::f64::consts::PI * (1.0 - ring as f64 / ALONG as f64);
            let r = if ring == 0 || ring == ALONG {
                0.0
            } else {
                radius * polar.sin()
            };
            (r, radius * polar.cos())
        })
        .collect();
    revolve(&rings)
}

/// A cylinder of `radius` and `height` along `axis`.
fn cylinder(radius: f64, height: f64, axis: &str) -> Tessellation {
    revolve(&[(radius, -height / 2.0), (radius, height / 2.0)]).orient(axis)
}

/// A cone of base `radius` and `height` along `axis`, apex at +height/2.
fn cone(radius: f64, height: f64, axis: &str) -> Tessellation {
    revolve(&[(radius, -height / 2.0), (0.0, height / 2.0)]).orient(axis)
}

/// A capsule: a cylinder of `height` with hemispheres of `radius` at its ends.
fn capsule(radius: f64, height: f64, axis: &str) -> Tessellation {
    let half = ALONG / 2;
    let mut rings = Vec::new();
    for ring in 0..=half {
        let polar = std::f64::consts::PI * (1.0 - ring as f64 / ALONG as f64);
        let r = if ring == 0 { 0.0 } else { radius * polar.sin() };
        rings.push((r, radius * polar.cos() - height / 2.0));
    }
    for ring in half..=ALONG {
        let polar = std::f64::consts::PI * (1.0 - ring as f64 / ALONG as f64);
        let r = if ring == ALONG {
            0.0
        } else {
            radius * polar.sin()
        };
        rings.push((r, radius * polar.cos() + height / 2.0));
    }
    revolve(&rings).orient(axis)
}

/// A plane of `width` (along the first axis) and `length`, facing `axis`.
fn plane(width: f64, length: f64, axis: &str) -> Tessellation {
    let (w, l) = (width / 2.0, length / 2.0);
    let mut t = Tessellation::new();
    t.points = vec![[-w, -l, 0.0], [w, -l, 0.0], [w, l, 0.0], [-w, l, 0.0]];
    t.face(&[0, 1, 2, 3]);
    t.orient(axis)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signed volume by the divergence theorem over fan triangles.
    fn volume(t: &Tessellation) -> f64 {
        let mut total = 0.0;
        let mut at = 0;
        for &count in &t.counts {
            let ids = &t.indices[at..at + count as usize];
            at += count as usize;
            let a = t.points[ids[0] as usize];
            for pair in ids[1..].windows(2) {
                let (b, c) = (t.points[pair[0] as usize], t.points[pair[1] as usize]);
                total += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]);
            }
        }
        total / 6.0
    }

    #[test]
    fn shapes_are_closed_and_outward() {
        assert!((volume(&cube(2.0)) - 8.0).abs() < 1e-12);
        let sphere_volume = volume(&sphere(1.0));
        assert!(sphere_volume > 3.9 && sphere_volume < 4.0 * std::f64::consts::PI / 3.0);
        for axis in ["X", "Y", "Z"] {
            assert!(volume(&cylinder(1.0, 2.0, axis)) > 6.0, "{axis}");
            assert!(volume(&cone(1.0, 3.0, axis)) > 3.0, "{axis}");
            assert!(volume(&capsule(0.5, 1.0, axis)) > 1.2, "{axis}");
        }
        assert_eq!(plane(2.0, 4.0, "Z").points[2], [1.0, 2.0, 0.0]);
    }
}
