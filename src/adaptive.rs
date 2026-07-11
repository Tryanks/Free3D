//! Selection-driven tool ranking behind the floating adaptive menu.
//!
//! Shapr3D's signature interaction: rather than hunting through tool groups,
//! the user selects geometry and the tools that apply to *that* selection come
//! to them, ranked. [`adaptive_tools`] is the pure ranking function; the chrome
//! in [`crate::ui::adaptive_menu`] renders whatever it returns.

use crate::{
    commands::ToolId,
    document::{BodyKind, Document, SelItem, Selection},
    sketch::SketchPlane,
};

/// Maximum number of entries the adaptive menu ever shows.
const MAX_ENTRIES: usize = 6;

/// Whether `tool` is wired up enough to offer adaptively.
///
/// Some ranking rules name tools that exist as [`ToolId`]s but have no dispatch
/// yet; those are ranked internally but filtered out so the menu never offers
/// no-op entries.
fn is_eligible(tool: ToolId) -> bool {
    matches!(
        tool,
        ToolId::Extrude
            | ToolId::OffsetFace
            | ToolId::Shell
            | ToolId::Revolve
            | ToolId::Sweep
            | ToolId::Loft
            | ToolId::Patch
            | ToolId::Stitch
            | ToolId::Thicken
            | ToolId::DeleteFace
            | ToolId::Line
            | ToolId::Fillet
            | ToolId::Chamfer
            | ToolId::Union
            | ToolId::Subtract
            | ToolId::Intersect
            | ToolId::Move
            | ToolId::Mirror
            | ToolId::Pattern
            | ToolId::Scale
            | ToolId::Split
            | ToolId::Align
            | ToolId::Hole
            | ToolId::Draft
            | ToolId::ReplaceFace
            | ToolId::Project
            | ToolId::Properties
            | ToolId::InterferenceCheck
            | ToolId::GeometryCheck
            | ToolId::Thread
            | ToolId::Ground
            | ToolId::Joint
            | ToolId::Drive
    )
}

/// Ranks the most relevant tools for the current `selection`.
///
/// Each present selection *kind* contributes an ordered candidate set; the
/// result is the intersection of those sets, ordered by the highest-priority
/// present kind (Profile > Face > Edge > Body), filtered to eligible tools and
/// capped at [`MAX_ENTRIES`]. Returns an empty vector for an empty selection or
/// when the selected kinds share no applicable tool.
pub fn adaptive_tools(selection: &Selection, document: &Document) -> Vec<ToolId> {
    if selection.items.is_empty() {
        return Vec::new();
    }

    let mut profiles = Vec::new();
    let mut sketch_entities = Vec::new();
    let mut face_bodies: Vec<(crate::document::BodyId, u32)> = Vec::new();
    let mut edge_bodies: Vec<(crate::document::BodyId, u32)> = Vec::new();
    let mut body_ids: Vec<crate::document::BodyId> = Vec::new();

    for item in &selection.items {
        match *item {
            SelItem::Profile(sketch, profile) => profiles.push((sketch, profile)),
            SelItem::SketchEntity(sketch, entity) => sketch_entities.push((sketch, entity)),
            SelItem::Face(id, index) => face_bodies.push((id, index)),
            SelItem::Edge(id, index) => edge_bodies.push((id, index)),
            SelItem::Body(id) => {
                if !body_ids.contains(&id) {
                    body_ids.push(id);
                }
            }
            SelItem::Plane(_) | SelItem::Axis(_) | SelItem::Point(_) => {}
        }
    }

    // Each entry is `(kind priority, ordered candidate tools)`; lower priority
    // sorts first and drives the final ordering.
    let mut sets: Vec<(u8, Vec<ToolId>)> = Vec::new();

    if !profiles.is_empty() {
        let mut tools = if profiles.len() >= 2 {
            vec![
                ToolId::Loft,
                ToolId::Sweep,
                ToolId::Extrude,
                ToolId::Revolve,
            ]
        } else if open_sweep_path_applies(selection, &sketch_entities, document) {
            vec![ToolId::Sweep, ToolId::Extrude, ToolId::Revolve]
        } else {
            vec![ToolId::Extrude, ToolId::Revolve]
        };
        tools.dedup();
        sets.push((0, tools));
    } else if open_chain_applies(&sketch_entities, document) {
        sets.push((0, vec![ToolId::Extrude, ToolId::Revolve]));
    }

    if face_selection_applies(&face_bodies, document) {
        let solid = document
            .bodies
            .iter()
            .find(|body| body.id == face_bodies[0].0)
            .is_some_and(|body| body.kind == BodyKind::Solid);
        sets.push((
            1,
            if solid {
                vec![
                    ToolId::Extrude,
                    ToolId::Hole,
                    ToolId::Draft,
                    ToolId::OffsetFace,
                    ToolId::ReplaceFace,
                    ToolId::Project,
                    ToolId::Shell,
                    ToolId::DeleteFace,
                    ToolId::Line,
                    ToolId::Revolve,
                ]
            } else {
                Vec::new()
            },
        ));
    } else if curved_face_selection_applies(&face_bodies, document) {
        // Curved faces cannot host sketches or planar push/pull, but shelling
        // with the face removed remains meaningful.
        let mut tools = vec![ToolId::Shell];
        if face_bodies.iter().all(|(id, index)| {
            document
                .bodies
                .iter()
                .find(|body| body.id == *id)
                .is_some_and(|body| {
                    body.shape.face_surface_kind(*index as usize).ok()
                        == Some(occt::SurfaceKind::Cylinder)
                })
        }) {
            tools.insert(0, ToolId::Thread);
        }
        sets.push((1, tools));
    }

    // Edges only rank when they all belong to a single body.
    if !edge_bodies.is_empty() && edge_bodies.iter().all(|(id, _)| *id == edge_bodies[0].0) {
        let mut tools = vec![ToolId::Fillet, ToolId::Chamfer];
        if closed_edge_loop_applies(&edge_bodies, document) {
            tools.insert(0, ToolId::Patch);
        }
        sets.push((2, tools));
    }

    match body_ids.len() {
        0 => {}
        1 => {
            let surface = document
                .bodies
                .iter()
                .find(|body| body.id == body_ids[0])
                .is_some_and(|body| body.kind == BodyKind::Surface);
            sets.push((
                3,
                if surface {
                    vec![
                        ToolId::Thicken,
                        ToolId::Move,
                        ToolId::Ground,
                        ToolId::Joint,
                        ToolId::Drive,
                        ToolId::Properties,
                        ToolId::GeometryCheck,
                        ToolId::Scale,
                        ToolId::Mirror,
                        ToolId::Pattern,
                    ]
                } else {
                    vec![
                        ToolId::Move,
                        ToolId::Ground,
                        ToolId::Joint,
                        ToolId::Drive,
                        ToolId::Shell,
                        ToolId::Properties,
                        ToolId::GeometryCheck,
                        ToolId::Scale,
                        ToolId::Mirror,
                        ToolId::Pattern,
                        ToolId::Split,
                    ]
                },
            ));
        }
        _ => {
            let all_surfaces = body_ids.iter().all(|id| {
                document
                    .bodies
                    .iter()
                    .find(|body| body.id == *id)
                    .is_some_and(|body| body.kind == BodyKind::Surface)
            });
            sets.push((
                3,
                if all_surfaces {
                    vec![
                        ToolId::Stitch,
                        ToolId::InterferenceCheck,
                        ToolId::Properties,
                        ToolId::Mirror,
                        ToolId::Pattern,
                        ToolId::Scale,
                        ToolId::Align,
                    ]
                } else {
                    vec![
                        ToolId::Union,
                        ToolId::InterferenceCheck,
                        ToolId::Properties,
                        ToolId::Subtract,
                        ToolId::Intersect,
                        ToolId::Mirror,
                        ToolId::Pattern,
                        ToolId::Scale,
                        ToolId::Align,
                    ]
                },
            ));
        }
    }

    if sets.is_empty() {
        return Vec::new();
    }
    sets.sort_by_key(|(priority, _)| *priority);

    // Intersect every set, preserving the highest-priority set's ordering.
    let ordering = sets[0].1.clone();
    let mut result: Vec<ToolId> = ordering
        .into_iter()
        .filter(|tool| sets.iter().all(|(_, set)| set.contains(tool)))
        .filter(|tool| is_eligible(*tool))
        .filter(|tool| *tool != ToolId::Project || document.active_sketch.is_some())
        .collect();
    result.truncate(MAX_ENTRIES);
    result
}

fn closed_edge_loop_applies(edges: &[(crate::document::BodyId, u32)], document: &Document) -> bool {
    let Some((body_id, _)) = edges.first() else {
        return false;
    };
    let Some(body) = document.bodies.iter().find(|body| body.id == *body_id) else {
        return false;
    };
    let endpoints: Vec<_> = edges
        .iter()
        .filter_map(|(_, edge)| {
            Some((
                body.shape.edge_start_point(*edge as usize).ok()?,
                body.shape.edge_end_point(*edge as usize).ok()?,
            ))
        })
        .collect();
    if endpoints.len() != edges.len() {
        return false;
    }
    endpoints.iter().flat_map(|(a, b)| [*a, *b]).all(|point| {
        endpoints
            .iter()
            .flat_map(|(a, b)| [*a, *b])
            .filter(|candidate| candidate.distance(point) < 1.0e-6)
            .count()
            == 2
    })
}

fn open_chain_applies(entities: &[(crate::sketch::SketchId, usize)], document: &Document) -> bool {
    let Some((sketch_id, _)) = entities.first() else {
        return false;
    };
    if entities.iter().any(|(id, _)| id != sketch_id) {
        return false;
    }
    let indices: Vec<_> = entities.iter().map(|(_, index)| *index).collect();
    document
        .sketches
        .iter()
        .find(|sketch| sketch.id == *sketch_id)
        .is_some_and(|sketch| {
            sketch.open_chains().iter().any(|chain| {
                chain.len() == indices.len() && chain.iter().all(|index| indices.contains(index))
            })
        })
}

fn open_sweep_path_applies(
    selection: &Selection,
    entities: &[(crate::sketch::SketchId, usize)],
    document: &Document,
) -> bool {
    let Some((sketch_id, _)) = entities.first() else {
        return false;
    };
    if selection.items.len() != entities.len() + 1 || entities.iter().any(|(id, _)| id != sketch_id)
    {
        return false;
    }
    let indices: Vec<_> = entities.iter().map(|(_, index)| *index).collect();
    document
        .sketches
        .iter()
        .find(|sketch| sketch.id == *sketch_id)
        .is_some_and(|sketch| {
            sketch.open_chains().iter().any(|chain| {
                chain.len() == indices.len() && chain.iter().all(|index| indices.contains(index))
            })
        })
}

/// Whether the selected faces all belong to one body and at least one is a
/// valid (non-planar) face — used to offer curved-face tools like Shell.
fn curved_face_selection_applies(
    faces: &[(crate::document::BodyId, u32)],
    document: &Document,
) -> bool {
    if faces.is_empty() {
        return false;
    }
    let first_body = faces[0].0;
    if faces.iter().any(|(id, _)| *id != first_body) {
        return false;
    }
    faces.iter().any(|(id, index)| {
        document
            .bodies
            .iter()
            .find(|body| body.id == *id)
            .is_some_and(|body| {
                body.kind == BodyKind::Solid
                    && body
                        .shape
                        .face_count()
                        .is_ok_and(|count| (*index as usize) < count)
            })
    })
}

/// Whether the selected faces qualify for face tools: at least one planar face,
/// all on the same body.
fn face_selection_applies(faces: &[(crate::document::BodyId, u32)], document: &Document) -> bool {
    if faces.is_empty() {
        return false;
    }
    let first_body = faces[0].0;
    if faces.iter().any(|(id, _)| *id != first_body) {
        return false;
    }
    faces.iter().any(|(id, index)| {
        document
            .bodies
            .iter()
            .find(|body| body.id == *id)
            .and_then(|body| SketchPlane::from_face(&body.shape, *index))
            .is_some()
    })
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use occt::Shape;

    use super::*;
    use crate::{
        document::{Document, SelItem, Selection},
        sketch::SketchEntity,
    };

    fn planar_top_face(document: &Document, body: crate::document::BodyId) -> u32 {
        let shape = &document
            .bodies
            .iter()
            .find(|candidate| candidate.id == body)
            .unwrap()
            .shape;
        (0..shape.face_count().expect("box face count"))
            .filter_map(|index| Some((index, shape.face_center_of_mass(index).ok()?)))
            .max_by(|(_, a), (_, b)| a.z.total_cmp(&b.z))
            .map(|(index, _)| index as u32)
            .expect("box has faces")
    }

    fn selection(items: Vec<SelItem>) -> Selection {
        Selection {
            items,
            ..Default::default()
        }
    }

    #[test]
    fn empty_selection_ranks_nothing() {
        let document = Document::new();
        assert!(adaptive_tools(&selection(Vec::new()), &document).is_empty());
    }

    #[test]
    fn planar_face_ranks_extrude_first() {
        let mut document = Document::new();
        let body = document.add_body("Box", Shape::cube(10.0));
        let face = planar_top_face(&document, body);
        let tools = adaptive_tools(&selection(vec![SelItem::Face(body, face)]), &document);
        assert_eq!(tools.first(), Some(&ToolId::Extrude));
        assert!(tools.contains(&ToolId::OffsetFace));
        assert!(tools.contains(&ToolId::Hole));
        assert!(tools.contains(&ToolId::Draft));
    }

    #[test]
    fn two_bodies_rank_union_first() {
        let mut document = Document::new();
        let a = document.add_body("A", Shape::cube(10.0));
        let b = document.add_body("B", Shape::cube(10.0));
        let tools = adaptive_tools(
            &selection(vec![SelItem::Body(a), SelItem::Body(b)]),
            &document,
        );
        assert_eq!(tools.first(), Some(&ToolId::Union));
        assert!(tools.contains(&ToolId::Subtract));
        assert!(tools.contains(&ToolId::Intersect));
    }

    #[test]
    fn edges_rank_fillet_first() {
        let mut document = Document::new();
        let body = document.add_body("Box", Shape::cube(10.0));
        let tools = adaptive_tools(&selection(vec![SelItem::Edge(body, 0)]), &document);
        assert_eq!(tools, vec![ToolId::Fillet, ToolId::Chamfer]);
    }

    #[test]
    fn profile_and_open_chain_rank_sweep() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            sketch,
            [
                SketchEntity::Circle {
                    center: glam::DVec2::ZERO,
                    radius: 2.0,
                },
                SketchEntity::Line {
                    a: glam::DVec2::ZERO,
                    b: glam::DVec2::new(10.0, 0.0),
                },
                SketchEntity::Line {
                    a: glam::DVec2::new(10.0, 0.0),
                    b: glam::DVec2::new(10.0, 20.0),
                },
            ]
        ));
        let tools = adaptive_tools(
            &selection(vec![
                SelItem::Profile(sketch, 0),
                SelItem::SketchEntity(sketch, 1),
                SelItem::SketchEntity(sketch, 2),
            ]),
            &document,
        );
        assert_eq!(tools.first(), Some(&ToolId::Sweep));
        assert!(!tools.contains(&ToolId::Loft));
    }

    #[test]
    fn multiple_profiles_rank_loft_then_sweep() {
        let mut document = Document::new();
        let first = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            first,
            [SketchEntity::Circle {
                center: glam::DVec2::ZERO,
                radius: 2.0,
            }]
        ));
        let second = document.add_sketch(SketchPlane {
            origin: DVec3::new(0.0, 0.0, 10.0),
            ..SketchPlane::xy()
        });
        assert!(document.add_sketch_entities(
            second,
            [SketchEntity::Circle {
                center: glam::DVec2::ZERO,
                radius: 3.0,
            }]
        ));
        let tools = adaptive_tools(
            &selection(vec![
                SelItem::Profile(first, 0),
                SelItem::Profile(second, 0),
            ]),
            &document,
        );
        assert_eq!(tools.get(0), Some(&ToolId::Loft));
        assert_eq!(tools.get(1), Some(&ToolId::Sweep));
    }

    #[test]
    fn single_body_ranks_move_first() {
        let mut document = Document::new();
        let body = document.add_body("Box", Shape::cube(10.0));
        let tools = adaptive_tools(&selection(vec![SelItem::Body(body)]), &document);
        assert_eq!(tools.first(), Some(&ToolId::Move));
        assert!(tools.contains(&ToolId::Shell));
    }

    #[test]
    fn mixed_body_and_face_intersect_to_shared_tool() {
        let mut document = Document::new();
        let body = document.add_body("Box", Shape::cube(10.0));
        let face = planar_top_face(&document, body);
        let tools = adaptive_tools(
            &selection(vec![SelItem::Body(body), SelItem::Face(body, face)]),
            &document,
        );
        // Shell is the only tool applicable to both a whole body and a face.
        assert_eq!(tools, vec![ToolId::Shell]);
    }

    #[test]
    fn ranking_is_capped_at_six() {
        let mut document = Document::new();
        let a = document.add_body(
            "A",
            Shape::box_from_corners(DVec3::ZERO, DVec3::splat(10.0)),
        );
        let b = document.add_body(
            "B",
            Shape::box_from_corners(DVec3::splat(5.0), DVec3::splat(15.0)),
        );
        for items in [
            vec![SelItem::Body(a)],
            vec![SelItem::Body(a), SelItem::Body(b)],
            vec![SelItem::Edge(a, 0), SelItem::Edge(a, 1)],
        ] {
            assert!(adaptive_tools(&selection(items), &document).len() <= MAX_ENTRIES);
        }
    }
}
