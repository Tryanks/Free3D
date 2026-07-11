//! CPU picking against attributed triangle meshes and projected BRep edges.

use glam::{DVec3, Mat4, Vec2, Vec3};

use crate::{camera::OrbitCamera, document::BodyId, kernel::BodyMesh};

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
    for body in bodies {
        for (edge_index, edge) in body.mesh.edges.iter().enumerate() {
            for segment in edge.points.windows(2) {
                let a = camera.project(body.pose.transform_point3(Vec3::from(segment[0])));
                let b = camera.project(body.pose.transform_point3(Vec3::from(segment[1])));
                let distance = point_segment_distance(cursor, a, b);
                if distance <= threshold && best.is_none_or(|current| distance < current.distance) {
                    best = Some(EdgeHit {
                        body: body.id,
                        edge: edge_index as u32,
                        distance,
                    });
                }
            }
        }
    }
    best
}

fn point_segment_distance(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let segment = b - a;
    let length_squared = segment.length_squared();
    if length_squared <= f32::EPSILON {
        return point.distance(a);
    }
    let t = ((point - a).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance(a + segment * t)
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
    fn edge_pick_respects_six_pixel_threshold() {
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
        assert!(pick_edge(&bodies, &camera, midpoint + normal * 5.5, 6.0).is_some());
        assert!(pick_edge(&bodies, &camera, midpoint + normal * 6.5, 6.0).is_none());
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
