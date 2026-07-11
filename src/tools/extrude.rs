//! Push/pull extrusion state, face analysis, and commit logic.

use glam::{DVec3, Vec3};
use occt::{Shape, SurfaceKind};
use serde::{Deserialize, Serialize};

use crate::{
    document::{BodyId, Document},
    gizmo::axis_drag_parameter,
    sketch::SketchId,
};

/// Boolean behavior applied when an extrusion is committed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "t")]
pub enum ExtrudeMode {
    /// Pull unions and push subtracts.
    #[default]
    Auto,
    /// Keeps the prism as a separate document body.
    NewBody,
    /// Fuses the prism with the selected body.
    Union,
    /// Cuts the prism from the selected body.
    Subtract,
    /// Keeps only the overlap with the selected body.
    Intersect,
}

/// Extent placement relative to the source sketch plane or face.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "t")]
pub enum ExtrudeSideMode {
    /// Extends only in the primary arrow direction.
    #[default]
    OneSided,
    /// Centers the requested total distance on the source plane.
    Symmetric,
    /// Uses independent primary and opposite distances.
    TwoSided,
}

impl ExtrudeSideMode {
    /// Modes in badge display order.
    pub const ALL: [Self; 3] = [Self::OneSided, Self::Symmetric, Self::TwoSided];

    /// Short viewport badge label.
    pub fn label(self) -> &'static str {
        match self {
            Self::OneSided => crate::i18n::t("One-Sided"),
            Self::Symmetric => crate::i18n::t("Symmetric"),
            Self::TwoSided => crate::i18n::t("Two-Sided"),
        }
    }
}

impl ExtrudeMode {
    /// Modes in badge display order.
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::NewBody,
        Self::Union,
        Self::Subtract,
        Self::Intersect,
    ];

    /// Short label shown in the viewport badge.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => crate::i18n::t("Auto"),
            Self::NewBody => crate::i18n::t("New Body"),
            Self::Union => crate::i18n::t("Union"),
            Self::Subtract => crate::i18n::t("Subtract"),
            Self::Intersect => crate::i18n::t("Intersect"),
        }
    }
}

/// State captured for one face-arrow drag.
#[derive(Clone, Copy, Debug)]
pub struct ExtrudeDrag {
    /// Body that owns the source face.
    pub body: BodyId,
    /// Source face in OCCT iteration order.
    pub face_index: u32,
    /// Face center in world coordinates.
    pub origin: DVec3,
    /// Cached outward unit normal.
    pub normal: DVec3,
    /// Signed distance along `normal`.
    pub distance: f64,
    /// Distance on the side opposite `normal` in two-sided mode.
    pub opposite_distance: f64,
    /// Placement of the extrusion relative to the source.
    pub side_mode: ExtrudeSideMode,
    /// Commit behavior.
    pub mode: ExtrudeMode,
}

/// State captured for one closed-sketch-profile extrusion.
#[derive(Clone, Copy, Debug)]
pub struct ProfileExtrudeDrag {
    /// Source sketch.
    pub sketch: SketchId,
    /// Profile index in the sketch's current detected profile order.
    pub profile_index: usize,
    /// Arrow origin in world coordinates.
    pub origin: DVec3,
    /// Plane normal and extrusion direction basis.
    pub normal: DVec3,
    /// Signed distance along `normal`.
    pub distance: f64,
    /// Distance on the side opposite `normal` in two-sided mode.
    pub opposite_distance: f64,
    /// Placement of the extrusion relative to the source.
    pub side_mode: ExtrudeSideMode,
    /// Commit behavior.
    pub mode: ExtrudeMode,
}

/// State captured for one open-sketch-chain surface extrusion.
#[derive(Clone, Debug)]
pub struct OpenChainExtrudeDrag {
    /// Source sketch.
    pub sketch: SketchId,
    /// Complete ordered open-chain entity selection.
    pub entity_indices: Vec<usize>,
    /// Arrow origin in world coordinates.
    pub origin: DVec3,
    /// Sketch-plane normal and extrusion direction basis.
    pub normal: DVec3,
    /// Signed distance along `normal`.
    pub distance: f64,
    /// Distance on the side opposite `normal` in two-sided mode.
    pub opposite_distance: f64,
    /// Placement of the extrusion relative to the source.
    pub side_mode: ExtrudeSideMode,
}

/// Effective boolean operation after resolving Auto from the drag sign.
pub fn resolved_mode(mode: ExtrudeMode, distance: f64) -> ExtrudeMode {
    if mode == ExtrudeMode::Auto {
        if distance >= 0.0 {
            ExtrudeMode::Union
        } else {
            ExtrudeMode::Subtract
        }
    } else {
        mode
    }
}

/// Signed cursor parameter along the extrusion axis.
pub fn cursor_distance(
    ray_origin: Vec3,
    ray_direction: Vec3,
    axis_origin: DVec3,
    axis: DVec3,
) -> f64 {
    f64::from(axis_drag_parameter(
        ray_origin,
        ray_direction,
        axis_origin.as_vec3(),
        axis.as_vec3(),
    ))
}

/// Returns a planar face center and its body-outward normal.
pub fn face_frame(shape: &Shape, face_index: u32) -> Option<(DVec3, DVec3)> {
    if shape.face_surface_kind(face_index as usize).ok()? != SurfaceKind::Plane {
        return None;
    }
    let origin = shape.face_center_of_mass(face_index as usize).ok()?;
    let mut normal = shape
        .face_normal_at(face_index as usize, origin)
        .ok()?
        .normalize_or_zero();
    if !origin.is_finite() || !normal.is_finite() || normal.length_squared() < 0.99 {
        return None;
    }
    let (minimum, maximum) = shape.aabb().ok()?;
    let diagonal = (maximum - minimum).length().max(1.0);
    let epsilon = diagonal * 1.0e-3;
    let sample = origin + normal * epsilon;
    let forward_hits = shape
        .ray_hits(sample, normal)
        .ok()?
        .into_iter()
        .filter(|hit| hit.t > epsilon * 0.1)
        .count();
    if forward_hits % 2 == 1 {
        normal = -normal;
    }
    Some((origin, normal))
}

/// Builds the signed-direction prism used by preview and commit.
pub fn prism(document: &Document, drag: &ExtrudeDrag) -> Option<Shape> {
    let (start, length) = extrusion_extents(drag.distance, drag.opposite_distance, drag.side_mode)?;
    if length.abs() < 1.0e-6 {
        return None;
    }
    let body = document.bodies.iter().find(|body| body.id == drag.body)?;
    let solid = body
        .shape
        .extrude_face(drag.face_index as usize, drag.normal * length)
        .ok()?;
    solid.translated(drag.normal * start).ok()
}

pub(crate) fn extrusion_extents(
    distance: f64,
    opposite_distance: f64,
    side_mode: ExtrudeSideMode,
) -> Option<(f64, f64)> {
    if !distance.is_finite() || !opposite_distance.is_finite() {
        return None;
    }
    Some(match side_mode {
        ExtrudeSideMode::OneSided => (0.0, distance),
        ExtrudeSideMode::Symmetric => (-distance.abs() * 0.5, distance.abs()),
        ExtrudeSideMode::TwoSided => {
            let a = distance.abs();
            let b = opposite_distance.abs();
            (-b, a + b)
        }
    })
}

/// Commits an extrusion as one undoable document mutation.
pub fn commit(document: &mut Document, drag: &ExtrudeDrag) -> bool {
    let Some(prism) = prism(document, drag) else {
        return false;
    };
    let Some(body) = document.bodies.iter().find(|body| body.id == drag.body) else {
        return false;
    };
    if body.kind != crate::document::BodyKind::Solid {
        return false;
    }
    let mode = resolved_mode(drag.mode, drag.distance);
    let result = match mode {
        ExtrudeMode::Union => body.shape.fuse(&prism).ok(),
        ExtrudeMode::Subtract => body.shape.cut(&prism).ok(),
        ExtrudeMode::Intersect => body.shape.common(&prism).ok(),
        ExtrudeMode::NewBody => None,
        ExtrudeMode::Auto => unreachable!("Auto is resolved above"),
    };
    document.apply_extrude_result(drag.body, prism, result)
}

/// Builds the signed-direction prism for a sketch profile.
pub fn profile_prism(document: &Document, drag: &ProfileExtrudeDrag) -> Option<Shape> {
    let (start, length) = extrusion_extents(drag.distance, drag.opposite_distance, drag.side_mode)?;
    if length.abs() < 1.0e-6 {
        return None;
    }
    let sketch = document
        .sketches
        .iter()
        .find(|sketch| sketch.id == drag.sketch)?;
    let profiles = sketch.profiles();
    let face = sketch.to_face(profiles.get(drag.profile_index)?)?;
    let solid = face
        .into_shape()
        .prism_of_face_shape(drag.normal * length)
        .ok()?;
    solid.translated(drag.normal * start).ok()
}

/// Builds the signed-direction shell for an open sketch chain.
pub fn open_chain_prism(document: &Document, drag: &OpenChainExtrudeDrag) -> Option<Shape> {
    let (start, length) = extrusion_extents(drag.distance, drag.opposite_distance, drag.side_mode)?;
    if length.abs() < 1.0e-6 {
        return None;
    }
    let sketch = document
        .sketches
        .iter()
        .find(|sketch| sketch.id == drag.sketch)?;
    let wire = sketch.open_chain_wire(&drag.entity_indices)?.into_shape();
    wire.prism_of_wire_shape(drag.normal * length)
        .ok()?
        .translated(drag.normal * start)
        .ok()
}

/// Commits a profile extrusion with simple first-intersection auto-boolean semantics.
pub fn commit_profile(document: &mut Document, drag: &ProfileExtrudeDrag) -> bool {
    let Some(prism) = profile_prism(document, drag) else {
        return false;
    };
    let support = document
        .sketches
        .iter()
        .find(|sketch| sketch.id == drag.sketch)
        .and_then(|sketch| sketch.support_body)
        .filter(|id| document.bodies.iter().any(|body| body.id == *id));
    let target = support.or_else(|| {
        document.bodies.iter().find_map(|body| {
            let common = body.shape.common(&prism).ok()?;
            (common.face_count().ok()? > 0 && common.mesh(0.5).is_ok()).then_some(body.id)
        })
    });
    let mode = if drag.mode == ExtrudeMode::Auto {
        match target {
            Some(_) if drag.distance >= 0.0 => ExtrudeMode::Union,
            Some(_) => ExtrudeMode::Subtract,
            None => ExtrudeMode::NewBody,
        }
    } else {
        drag.mode
    };
    let replacement = target.and_then(|id| {
        let body = document.bodies.iter().find(|body| body.id == id)?;
        Some(match mode {
            ExtrudeMode::Union => body.shape.fuse(&prism).ok()?,
            ExtrudeMode::Subtract => body.shape.cut(&prism).ok()?,
            ExtrudeMode::Intersect => body.shape.common(&prism).ok()?,
            ExtrudeMode::NewBody | ExtrudeMode::Auto => return None,
        })
    });
    document.apply_profile_extrude_result(target, prism, replacement)
}

#[cfg(test)]
mod tests {
    use glam::DVec2;
    use occt::Shape;

    use super::*;
    use crate::sketch::{SketchEntity, SketchPlane};

    struct TestBounds {
        minimum: DVec3,
        maximum: DVec3,
    }
    impl TestBounds {
        fn min(&self) -> DVec3 {
            self.minimum
        }
        fn max(&self) -> DVec3 {
            self.maximum
        }
    }
    fn aabb(shape: &Shape) -> TestBounds {
        let (minimum, maximum) = shape.aabb().expect("test bounds");
        TestBounds { minimum, maximum }
    }

    fn box_document() -> (Document, BodyId) {
        let mut document = Document::new();
        let id = document.add_body("Box 1", Shape::cube(10.0));
        (document, id)
    }

    fn horizontal_face(shape: &Shape, top: bool) -> (u32, DVec3, DVec3) {
        (0..shape.face_count().unwrap())
            .filter_map(|index| {
                let center = shape.face_center_of_mass(index).ok()?;
                face_frame(shape, index as u32).map(|(_, normal)| (index as u32, center, normal))
            })
            .find(|(_, center, _)| {
                if top {
                    (center.z - 10.0).abs() < 1.0e-6
                } else {
                    center.z.abs() < 1.0e-6
                }
            })
            .expect("box horizontal face")
    }

    #[test]
    fn box_top_and_bottom_normals_point_outward() {
        let shape = Shape::cube(10.0).unwrap();
        let (_, _, top) = horizontal_face(&shape, true);
        let (_, _, bottom) = horizontal_face(&shape, false);
        assert!(top.dot(DVec3::Z) > 0.999);
        assert!(bottom.dot(DVec3::NEG_Z) > 0.999);
    }

    #[test]
    fn auto_mode_follows_distance_sign() {
        assert_eq!(resolved_mode(ExtrudeMode::Auto, 2.0), ExtrudeMode::Union);
        assert_eq!(
            resolved_mode(ExtrudeMode::Auto, -2.0),
            ExtrudeMode::Subtract
        );
        assert_eq!(
            resolved_mode(ExtrudeMode::Intersect, -2.0),
            ExtrudeMode::Intersect
        );
    }

    #[test]
    fn union_grows_bbox_and_subtract_reduces_bbox() {
        let (mut document, id) = box_document();
        let (top_index, top_origin, top_normal) = horizontal_face(&document.bodies[0].shape, true);
        assert!(commit(
            &mut document,
            &ExtrudeDrag {
                body: id,
                face_index: top_index,
                origin: top_origin,
                normal: top_normal,
                distance: 5.0,
                opposite_distance: 0.0,
                side_mode: ExtrudeSideMode::OneSided,
                mode: ExtrudeMode::Auto,
            }
        ));
        let grown = aabb(&document.bodies[0].shape);
        assert!((grown.max().z - 15.0).abs() < 1.0e-5);

        let (mut document, id) = box_document();
        let (top_index, top_origin, top_normal) = horizontal_face(&document.bodies[0].shape, true);
        assert!(commit(
            &mut document,
            &ExtrudeDrag {
                body: id,
                face_index: top_index,
                origin: top_origin,
                normal: top_normal,
                distance: -4.0,
                opposite_distance: 0.0,
                side_mode: ExtrudeSideMode::OneSided,
                mode: ExtrudeMode::Auto,
            }
        ));
        let cut = aabb(&document.bodies[0].shape);
        assert!((cut.max().z - 6.0).abs() < 1.0e-5);
    }

    #[test]
    fn zero_distance_does_not_push_snapshot() {
        let (mut document, id) = box_document();
        let (face_index, origin, normal) = horizontal_face(&document.bodies[0].shape, true);
        assert!(!commit(
            &mut document,
            &ExtrudeDrag {
                body: id,
                face_index,
                origin,
                normal,
                distance: 0.0,
                opposite_distance: 0.0,
                side_mode: ExtrudeSideMode::OneSided,
                mode: ExtrudeMode::Auto,
            }
        ));
        // The only undo entry is the initial add; no extrusion snapshot exists.
        assert!(document.undo());
        assert!(document.bodies.is_empty());
        assert!(!document.undo());
    }

    #[test]
    fn closed_sketch_profile_extrudes_to_new_body() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        let points = [
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 8.0),
            DVec2::new(0.0, 8.0),
        ];
        document.add_sketch_entities(
            sketch,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            }),
        );
        assert!(commit_profile(
            &mut document,
            &ProfileExtrudeDrag {
                sketch,
                profile_index: 0,
                origin: DVec3::new(5.0, 4.0, 0.0),
                normal: DVec3::Z,
                distance: 5.0,
                opposite_distance: 0.0,
                side_mode: ExtrudeSideMode::OneSided,
                mode: ExtrudeMode::Auto,
            }
        ));
        assert_eq!(document.bodies.len(), 1);
        let bounds = aabb(&document.bodies[0].shape);
        assert!((bounds.max() - DVec3::new(10.0, 8.0, 5.0)).length() < 1.0e-5);
    }

    #[test]
    fn symmetric_profile_bbox_spans_both_sides_of_sketch_plane() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        let points = [
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 8.0),
            DVec2::new(0.0, 8.0),
        ];
        document.add_sketch_entities(
            sketch,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            }),
        );
        let drag = ProfileExtrudeDrag {
            sketch,
            profile_index: 0,
            origin: DVec3::new(5.0, 4.0, 0.0),
            normal: DVec3::Z,
            distance: 20.0,
            opposite_distance: 0.0,
            side_mode: ExtrudeSideMode::Symmetric,
            mode: ExtrudeMode::NewBody,
        };
        let bounds = aabb(&profile_prism(&document, &drag).expect("symmetric prism"));
        assert!((bounds.min().z + 10.0).abs() < 1.0e-5);
        assert!((bounds.max().z - 10.0).abs() < 1.0e-5);
    }
    #[test]
    fn sphere_face_frame_returns_none_without_aborting() {
        use glam::dvec3;
        let shape = Shape::sphere(dvec3(0.0, 0.0, 30.0), 30.0).unwrap();
        // A spherical face is not planar; before the boundary-probe fix this
        // call aborted the process via an OCCT Standard_OutOfRange throw.
        assert!(face_frame(&shape, 0).is_none());
    }
}
