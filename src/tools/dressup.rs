//! Edge fillet/chamfer geometry and arrow placement.

use glam::DVec3;
use occt::Shape;

use crate::document::{BodyId, Document};

/// State captured for one edge dress-up drag.
#[derive(Clone, Debug)]
pub struct DressUpDrag {
    /// Body owning every selected edge.
    pub body: BodyId,
    /// Selected edges in OCCT iteration order.
    pub edge_indices: Vec<u32>,
    /// First selected edge midpoint.
    pub origin: DVec3,
    /// Outward adjacent-face bisector.
    pub direction: DVec3,
    /// Fillet radius or chamfer distance.
    pub radius: f64,
    /// Optional end radius for a linear variable fillet.
    pub end_radius: Option<f64>,
    /// Kind of edge treatment.
    pub fillet: bool,
}

/// Returns the midpoint and average adjacent-face normal for an edge.
pub fn edge_frame(shape: &Shape, edge_index: u32) -> Option<(DVec3, DVec3)> {
    let points = shape.edge_polyline(edge_index as usize, 0.1).ok()?;
    let total_length: f64 = points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum();
    let mut remaining = total_length * 0.5;
    let mut origin = (shape.edge_start_point(edge_index as usize).ok()?
        + shape.edge_end_point(edge_index as usize).ok()?)
        * 0.5;
    for pair in points.windows(2) {
        let segment_length = pair[0].distance(pair[1]);
        if remaining <= segment_length && segment_length > f64::EPSILON {
            origin = pair[0].lerp(pair[1], remaining / segment_length);
            break;
        }
        remaining -= segment_length;
    }
    let mut normals = Vec::new();
    for face_index in 0..shape.face_count().ok()? {
        if shape
            .face_contains_edge(face_index, edge_index as usize)
            .ok()?
        {
            let mut normal = shape
                .face_normal_at(face_index, origin)
                .ok()?
                .normalize_or_zero();
            if !normal.is_finite() || normal.length_squared() < 0.99 {
                continue;
            }
            // Match `face_frame`'s inside/outside parity test without requiring
            // the adjacent face itself to be planar.
            let epsilon = 1.0e-3;
            let hits = shape
                .ray_hits(origin + normal * epsilon, normal)
                .ok()?
                .into_iter()
                .filter(|hit| hit.t > epsilon * 0.1)
                .count();
            if hits % 2 == 1 {
                normal = -normal;
            }
            normals.push(normal);
        }
    }
    let direction = normals
        .into_iter()
        .fold(DVec3::ZERO, |sum, normal| sum + normal)
        .normalize_or_zero();
    (origin.is_finite() && direction.length_squared() > 0.5).then_some((origin, direction))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_edge_frame_has_finite_outward_bisector() {
        let shape = Shape::cube(10.0).unwrap();
        let (origin, direction) = edge_frame(&shape, 0).expect("cube edge frame");
        assert!(origin.is_finite());
        assert!((direction.length() - 1.0).abs() < 1.0e-6);
    }
}

/// Builds a whole-body fillet/chamfer result for preview.
pub fn preview(document: &Document, drag: &DressUpDrag) -> Option<Shape> {
    let body = document.bodies.iter().find(|body| body.id == drag.body)?;
    if drag.edge_indices.is_empty()
        || drag
            .edge_indices
            .iter()
            .any(|&index| index as usize >= body.shape.edge_count().ok().unwrap_or(0))
    {
        return None;
    }
    let shape = if drag.fillet {
        if let Some(end) = drag.end_radius {
            body.shape
                .variable_fillet_edges(&drag.edge_indices, drag.radius, end)
                .ok()?
        } else {
            body.shape
                .fillet_edges(drag.radius, &drag.edge_indices)
                .ok()?
        }
    } else {
        body.shape
            .chamfer_edges(drag.radius, &drag.edge_indices)
            .ok()?
    };
    shape.mesh(0.5).ok()?;
    (shape.face_count().ok()? > 0).then_some(shape)
}
