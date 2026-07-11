//! Planar-face offset implemented through push/pull extrusion semantics.

use crate::{
    document::Document,
    tools::extrude::{self, ExtrudeDrag, ExtrudeMode},
};

/// Commits a planar-face offset with automatic union/subtraction.
///
/// The caller obtains the planar frame through [`extrude::face_frame`]. This
/// wrapper deliberately ignores any mode carried by the shared drag state so
/// Offset Face cannot create a separate body or expose boolean overrides.
pub fn commit(document: &mut Document, drag: &ExtrudeDrag) -> bool {
    let mut automatic = *drag;
    automatic.mode = ExtrudeMode::Auto;
    extrude::commit(document, &automatic)
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use occt::Shape;

    use super::*;
    use crate::{document::Document, tools::extrude::face_frame};

    fn top_face(shape: &Shape) -> (u32, DVec3, DVec3) {
        (0..shape.face_count().unwrap())
            .filter_map(|index| {
                let center = shape.face_center_of_mass(index).ok()?;
                face_frame(shape, index as u32).map(|(_, normal)| (index as u32, center, normal))
            })
            .find(|(_, center, _)| (center.z - 10.0).abs() < 1.0e-6)
            .expect("box top face")
    }

    #[test]
    fn offset_face_grows_and_shrinks_bbox_with_auto_semantics() {
        for (distance, expected_max_z) in [(5.0, 15.0), (-4.0, 6.0)] {
            let mut document = Document::new();
            let id = document.add_body("Box", Shape::cube(10.0));
            let (face_index, origin, normal) = top_face(&document.bodies[0].shape);
            assert!(commit(
                &mut document,
                &ExtrudeDrag {
                    body: id,
                    face_index,
                    origin,
                    normal,
                    distance,
                    opposite_distance: 0.0,
                    side_mode: crate::tools::extrude::ExtrudeSideMode::OneSided,
                    mode: ExtrudeMode::NewBody,
                }
            ));
            let (_, maximum) = document.bodies[0].shape.aabb().unwrap();
            assert!((maximum.z - expected_max_z).abs() < 1.0e-5);
            assert_eq!(document.bodies.len(), 1);
        }
    }
}
