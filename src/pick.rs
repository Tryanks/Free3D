//! CPU picking against attributed triangle meshes and projected BRep edges.

use glam::{DVec3, Mat4, Vec2, Vec3};

use crate::{camera::OrbitCamera, document::BodyId, kernel::BodyMesh};

/// Logical-pixel radius used for body-edge picking.
pub const EDGE_PICK_THRESHOLD_PX: f32 = 8.0;

/// Fraction of the scene diagonal tolerated between a face and an edge at the
/// same visible surface.
pub const EDGE_OCCLUSION_EPSILON_FRACTION: f32 = 2.0e-3;

/// A visible body participating in a pick query.
pub struct PickBody<'a> {
    /// Owning document id.
    pub id: BodyId,
    /// Tessellated BRep body.
    pub mesh: &'a BodyMesh,
    /// Exact BRep used for face intersection.
    pub shape: &'a occt::Shape,
    /// Runtime assembly/exploded model transform.
    pub pose: Mat4,
}

/// One attributed face intersection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceHit {
    /// Owning body.
    pub body: BodyId,
    /// Zero-based BRep face index.
    pub face: u32,
    /// Ray distance.
    pub t: f32,
}

/// Finds every exact BRep face hit, sorted from nearest to farthest.
///
/// OpenCASCADE may report the same attributed face more than once where a ray
/// crosses a tessellation seam. Those duplicates are collapsed while the
/// nearest distance for each `(body, face)` pair is retained.
pub fn pick_all(bodies: &[PickBody<'_>], origin: Vec3, ray: Vec3) -> Vec<FaceHit> {
    let mut hits = Vec::new();
    for body in bodies {
        let inverse = body.pose.inverse();
        let local_origin = inverse.transform_point3(origin);
        let local_ray = inverse.transform_vector3(ray).normalize_or_zero();
        let Ok(body_hits) = body
            .shape
            .ray_hits(local_origin.as_dvec3(), local_ray.as_dvec3())
        else {
            continue;
        };
        for hit in body_hits {
            let world_hit = body
                .pose
                .transform_point3(local_origin + local_ray * hit.t as f32);
            let t = world_hit.distance(origin);
            if t.is_finite() && t >= 0.0 {
                hits.push(FaceHit {
                    body: body.id,
                    face: hit.face_index as u32,
                    t,
                });
            }
        }
    }
    hits.sort_by(|left, right| left.t.total_cmp(&right.t));
    let mut seen = std::collections::HashSet::new();
    hits.retain(|hit| seen.insert((hit.body, hit.face)));
    hits
}

/// Nearest screen-space edge hit.
#[derive(Clone, Copy, Debug)]
pub struct EdgeHit {
    /// Owning body.
    pub body: BodyId,
    /// Zero-based BRep edge index.
    pub edge: u32,
    /// Cursor-to-edge distance in device pixels.
    pub distance: f32,
    /// Depth of the closest edge point along the cursor ray.
    pub t: f32,
    /// Closest point on the projected edge, in viewport pixels.
    pub screen: Vec2,
    /// Perspective-correct world position of that closest point.
    pub world: Vec3,
}

/// Rejects an edge candidate that is hidden behind a face.
///
/// Depths are compared along a single ray cast through the edge's closest
/// screen point, so perspective cannot skew the comparison the way two
/// different rays would.
pub fn edge_wins_over_face(
    bodies: &[PickBody<'_>],
    camera: &OrbitCamera,
    edge: Option<EdgeHit>,
    scene_diagonal: f32,
) -> Option<EdgeHit> {
    let edge = edge?;
    let (origin, ray) = camera.unproject_ray(edge.screen);
    let edge_t = (edge.world - origin).dot(ray);
    let epsilon = scene_diagonal.max(1.0e-4) * EDGE_OCCLUSION_EPSILON_FRACTION;
    pick_face(bodies, origin, ray)
        .is_none_or(|face| edge_t <= face.t + epsilon)
        .then_some(edge)
}

/// Fits an infinite line through an attributed edge polyline.
///
/// The most widely separated sample pair defines the line. Curved edges are
/// rejected when any sample deviates by more than `1e-4` of that span.
pub fn fit_straight_edge(points: &[[f32; 3]]) -> Option<(DVec3, DVec3)> {
    let (origin, endpoint, length) = points
        .iter()
        .enumerate()
        .flat_map(|(index, &a)| points[index + 1..].iter().map(move |&b| (a, b)))
        .map(|(a, b)| {
            let a = Vec3::from(a).as_dvec3();
            let b = Vec3::from(b).as_dvec3();
            (a, b, a.distance(b))
        })
        .max_by(|left, right| left.2.total_cmp(&right.2))?;
    if !length.is_finite() || length <= f64::EPSILON {
        return None;
    }
    let direction = (endpoint - origin) / length;
    let tolerance = length * 1.0e-4;
    points
        .iter()
        .map(|&point| {
            (Vec3::from(point).as_dvec3() - origin)
                .cross(direction)
                .length()
        })
        .all(|deviation| deviation <= tolerance)
        .then_some((origin, direction))
}

/// Finds the nearest exact BRep ray hit and retains its face attribution.
pub fn pick_face(bodies: &[PickBody<'_>], origin: Vec3, ray: Vec3) -> Option<FaceHit> {
    pick_all(bodies, origin, ray).into_iter().next()
}

/// Picks the closest projected polyline segment within `threshold` pixels.
pub fn pick_edge(
    bodies: &[PickBody<'_>],
    camera: &OrbitCamera,
    cursor: Vec2,
    threshold: f32,
) -> Option<EdgeHit> {
    let mut best: Option<EdgeHit> = None;
    let (ray_origin, ray) = camera.unproject_ray(cursor);
    for body in bodies {
        for (edge_index, edge) in body.mesh.edges.iter().enumerate() {
            for segment in edge.points.windows(2) {
                let world_a = body.pose.transform_point3(Vec3::from(segment[0]));
                let world_b = body.pose.transform_point3(Vec3::from(segment[1]));
                let (a, w_a) = camera.project_with_w(world_a);
                let (b, w_b) = camera.project_with_w(world_b);
                let (distance, screen_t) = point_segment_distance_and_t(cursor, a, b);
                if distance <= threshold && best.is_none_or(|current| distance < current.distance) {
                    let world_t = perspective_correct_t(screen_t, w_a, w_b);
                    let closest = world_a.lerp(world_b, world_t);
                    best = Some(EdgeHit {
                        body: body.id,
                        edge: edge_index as u32,
                        distance,
                        t: (closest - ray_origin).dot(ray),
                        screen: a + (b - a) * screen_t,
                        world: closest,
                    });
                }
            }
        }
    }
    best
}

/// Converts a screen-space interpolation parameter along a projected segment
/// into the corresponding world-space parameter (perspective-correct).
fn perspective_correct_t(screen_t: f32, w_a: f32, w_b: f32) -> f32 {
    let denominator = (1.0 - screen_t) * w_b + screen_t * w_a;
    if denominator.abs() <= f32::EPSILON {
        return screen_t;
    }
    (screen_t * w_a / denominator).clamp(0.0, 1.0)
}

fn point_segment_distance_and_t(point: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let segment = b - a;
    let length_squared = segment.length_squared();
    if length_squared <= f32::EPSILON {
        return (point.distance(a), 0.0);
    }
    let t = ((point - a).dot(segment) / length_squared).clamp(0.0, 1.0);
    (point.distance(a + segment * t), t)
}

#[cfg(test)]
mod tests {
    use occt::Shape;

    use crate::kernel::tessellate;

    use super::*;

    #[test]
    fn ray_hits_the_top_face_of_a_unit_box() {
        let shape = Shape::cube(1.0).unwrap();
        let mesh = tessellate(&shape, 0.1);
        let expected = mesh
            .face_ranges
            .iter()
            .position(|range| {
                mesh.indices[range.start as usize..range.end as usize]
                    .iter()
                    .all(|&index| (mesh.positions[index as usize][2] - 1.0).abs() < 1.0e-4)
            })
            .expect("unit box must have a top face");
        let bodies = [PickBody {
            id: BodyId(7),
            mesh: &mesh,
            shape: &shape,
            pose: Mat4::IDENTITY,
        }];
        let hit = pick_face(&bodies, Vec3::new(0.5, 0.5, 2.0), Vec3::NEG_Z).unwrap();
        assert_eq!(hit.body, BodyId(7));
        assert_eq!(hit.face, expected as u32);
        assert!((hit.t - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn pick_all_orders_hits_and_deduplicates_attributed_faces() {
        let near = Shape::cube(1.0).unwrap();
        let far = Shape::cube(1.0)
            .unwrap()
            .translated(DVec3::new(0.0, 0.0, -3.0))
            .unwrap();
        let near_mesh = tessellate(&near, 0.1);
        let far_mesh = tessellate(&far, 0.1);
        let bodies = [
            PickBody {
                id: BodyId(1),
                mesh: &far_mesh,
                shape: &far,
                pose: Mat4::IDENTITY,
            },
            PickBody {
                id: BodyId(2),
                mesh: &near_mesh,
                shape: &near,
                pose: Mat4::IDENTITY,
            },
        ];

        let hits = pick_all(&bodies, Vec3::new(0.5, 0.5, 2.0), Vec3::NEG_Z);
        assert!(
            hits.len() >= 4,
            "two boxes should contribute entry and exit faces"
        );
        assert!(hits.windows(2).all(|pair| pair[0].t <= pair[1].t));
        assert_eq!(hits[0].body, BodyId(2));
        let unique: std::collections::HashSet<_> =
            hits.iter().map(|hit| (hit.body, hit.face)).collect();
        assert_eq!(unique.len(), hits.len());
    }

    #[test]
    fn edge_pick_respects_eight_pixel_threshold() {
        let camera = OrbitCamera::new(Vec3::ZERO, 10.0, Vec2::new(800.0, 600.0));
        let mut mesh = BodyMesh::default();
        mesh.edges.push(crate::kernel::EdgePolyline {
            points: vec![[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
            range: 0..2,
        });
        let shape = Shape::cube(1.0).unwrap();
        let bodies = [PickBody {
            id: BodyId(1),
            mesh: &mesh,
            shape: &shape,
            pose: Mat4::IDENTITY,
        }];
        let a = camera.project(Vec3::new(-1.0, 0.0, 0.0));
        let b = camera.project(Vec3::new(1.0, 0.0, 0.0));
        let midpoint = (a + b) * 0.5;
        let direction = (b - a).normalize();
        let normal = Vec2::new(-direction.y, direction.x);
        assert!(
            pick_edge(
                &bodies,
                &camera,
                midpoint + normal * (EDGE_PICK_THRESHOLD_PX - 0.5),
                EDGE_PICK_THRESHOLD_PX,
            )
            .is_some()
        );
        assert!(
            pick_edge(
                &bodies,
                &camera,
                midpoint + normal * (EDGE_PICK_THRESHOLD_PX + 0.5),
                EDGE_PICK_THRESHOLD_PX,
            )
            .is_none()
        );
    }

    #[test]
    fn edge_priority_rejects_occluded_edges_and_preserves_faces_without_edges() {
        let camera = OrbitCamera::new(Vec3::ZERO, 10.0, Vec2::new(800.0, 600.0));
        let mesh = tessellate(&Shape::cube(2.0).unwrap(), 0.1);
        let shape = Shape::cube(2.0).unwrap();
        let bodies = [PickBody {
            id: BodyId(1),
            mesh: &mesh,
            shape: &shape,
            pose: Mat4::IDENTITY,
        }];
        let (origin, ray) = camera.unproject_ray(Vec2::new(400.0, 300.0));
        let front = pick_face(&bodies, origin, ray).expect("cube under cursor");
        let front_point = origin + ray * front.t;
        let hit = |world: Vec3| EdgeHit {
            body: BodyId(1),
            edge: 2,
            distance: EDGE_PICK_THRESHOLD_PX - 1.0,
            t: (world - origin).dot(ray),
            screen: camera.project(world),
            world,
        };
        // A point on the visible surface survives the occlusion test.
        let visible = hit(front_point);
        assert_eq!(
            edge_wins_over_face(&bodies, &camera, Some(visible), 10.0).map(|h| h.edge),
            Some(2)
        );
        // The same screen position but on the cube's far side is rejected.
        let hidden = hit(front_point + ray * 2.0);
        assert!(edge_wins_over_face(&bodies, &camera, Some(hidden), 10.0).is_none());
        // No candidate stays no candidate.
        assert!(edge_wins_over_face(&bodies, &camera, None, 10.0).is_none());
        // Without any occluder every candidate survives.
        assert!(edge_wins_over_face(&[], &camera, Some(hidden), 10.0).is_some());
    }

    #[test]
    fn straight_edge_fit_accepts_box_edge_and_rejects_cylinder_rim() {
        let box_mesh = tessellate(&Shape::cube(10.0).unwrap(), 0.1);
        assert!(
            box_mesh
                .edges
                .iter()
                .any(|edge| fit_straight_edge(&edge.points).is_some())
        );

        let cylinder = Shape::cylinder(DVec3::ZERO, 5.0, DVec3::Z, 10.0).unwrap();
        let cylinder_mesh = tessellate(&cylinder, 0.1);
        let rim = cylinder_mesh
            .edges
            .iter()
            .find(|edge| edge.points.len() > 4)
            .expect("cylinder must tessellate a curved rim");
        assert!(fit_straight_edge(&rim.points).is_none());
    }
    #[test]
    fn sphere_face_ray_does_not_throw() {
        use glam::dvec3;
        let shape = Shape::sphere(dvec3(0.0, 0.0, 30.0), 30.0).unwrap();
        let origin = dvec3(95.0, -95.0, 110.0);
        let dir = (dvec3(0.0, 0.0, 30.0) - origin).normalize();
        let hits = shape.ray_hits(origin, dir).unwrap();
        assert!(!hits.is_empty());
    }
}
