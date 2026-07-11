//! OpenCASCADE isolation layer for renderable geometry.

use std::ops::Range;

use occt::Shape;

/// One sampled BRep edge and its range in the body's flat line vertex stream.
#[derive(Clone, Debug)]
pub struct EdgePolyline {
    /// Ordered samples along the BRep curve.
    pub points: Vec<[f32; 3]>,
    /// Line-list vertex range in [`BodyMesh::edge_vertices`].
    pub range: Range<u32>,
}

/// GPU-ready body mesh with stable BRep face and edge attribution.
#[derive(Clone, Debug, Default)]
pub struct BodyMesh {
    /// Face-local positions concatenated in OCCT face iteration order.
    pub positions: Vec<[f32; 3]>,
    /// Per-position smooth normals (face-local, preserving BRep creases).
    pub normals: Vec<[f32; 3]>,
    /// Optional normalized mean-curvature proxy, populated only for analysis.
    pub curvature: Option<Vec<f32>>,
    /// Triangle indices into `positions`.
    pub indices: Vec<u32>,
    /// Index ranges parallel to `shape.faces()`.
    pub face_ranges: Vec<Range<u32>>,
    /// Edge samples parallel to `shape.edges()`.
    pub edges: Vec<EdgePolyline>,
    /// Flat line-list vertices used by the renderer.
    pub edge_vertices: Vec<[f32; 3]>,
}

impl BodyMesh {
    /// Lazily computes a finite per-vertex normal-variation curvature proxy.
    ///
    /// For each indexed triangle edge, this accumulates the difference between
    /// endpoint normals and divides by edge length. It is inexpensive, stable
    /// for OCCT's face-local vertices, and intentionally approximate.
    pub fn ensure_curvature(&mut self) {
        if self.curvature.is_some() {
            return;
        }
        let mut sum = vec![0.0_f32; self.positions.len()];
        let mut count = vec![0_u32; self.positions.len()];
        for triangle in self.indices.chunks_exact(3) {
            for (a, b) in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                let (a, b) = (a as usize, b as usize);
                let Some((&pa, &pb, &na, &nb)) = self
                    .positions
                    .get(a)
                    .zip(self.positions.get(b))
                    .zip(self.normals.get(a))
                    .zip(self.normals.get(b))
                    .map(|(((pa, pb), na), nb)| (pa, pb, na, nb))
                else {
                    continue;
                };
                let edge =
                    ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2))
                        .sqrt()
                        .max(1.0e-6);
                let variation =
                    ((na[0] - nb[0]).powi(2) + (na[1] - nb[1]).powi(2) + (na[2] - nb[2]).powi(2))
                        .sqrt()
                        / edge;
                if variation.is_finite() {
                    sum[a] += variation;
                    sum[b] += variation;
                    count[a] += 1;
                    count[b] += 1;
                }
            }
        }
        let mut values: Vec<f32> = sum
            .into_iter()
            .zip(count)
            .map(|(sum, count)| if count == 0 { 0.0 } else { sum / count as f32 })
            .collect();
        let maximum = values.iter().copied().fold(0.0_f32, f32::max).max(1.0e-6);
        for value in &mut values {
            *value = (*value / maximum).clamp(0.0, 1.0);
        }
        self.curvature = Some(values);
    }
}

/// Tessellates the complete BRep once, then samples every BRep edge.
pub fn tessellate(shape: &Shape, tolerance: f64) -> BodyMesh {
    let mesh = shape
        .mesh(tolerance)
        .expect("OpenCASCADE failed to tessellate a BRep shape");
    let mut result = BodyMesh {
        positions: mesh
            .positions
            .into_iter()
            .map(|point| [point.x as f32, point.y as f32, point.z as f32])
            .collect(),
        normals: mesh
            .normals
            .into_iter()
            .map(|normal| [normal.x as f32, normal.y as f32, normal.z as f32])
            .collect(),
        indices: mesh.indices,
        face_ranges: mesh.face_ranges,
        ..BodyMesh::default()
    };

    for edge_index in 0..shape.edge_count().expect("failed to count BRep edges") {
        let points: Vec<[f32; 3]> = shape
            .edge_polyline(edge_index, tolerance)
            .expect("OpenCASCADE failed to sample a BRep edge")
            .into_iter()
            .map(|point| [point.x as f32, point.y as f32, point.z as f32])
            .collect();
        let start = result.edge_vertices.len() as u32;
        for segment in points.windows(2) {
            result.edge_vertices.extend_from_slice(segment);
        }
        result.edges.push(EdgePolyline {
            points,
            range: start..result.edge_vertices.len() as u32,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_mesh_has_six_attributed_faces() {
        let mesh = tessellate(
            &Shape::box_from_corners(glam::DVec3::ZERO, glam::DVec3::ONE).unwrap(),
            0.1,
        );
        assert_eq!(mesh.face_ranges.len(), 6);
        assert!(mesh.face_ranges.iter().all(|range| !range.is_empty()));
        assert!(!mesh.edges.is_empty());
    }

    #[test]
    fn sphere_curvature_buffer_is_non_empty_and_finite() {
        // An octahedral sphere keeps this CPU-only analysis test from adding
        // concurrent calls into OCCT's process-global mesher.
        let positions = vec![
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        let mut mesh = BodyMesh {
            normals: positions.clone(),
            positions,
            indices: vec![
                4, 0, 2, 4, 2, 1, 4, 1, 3, 4, 3, 0, 5, 2, 0, 5, 1, 2, 5, 3, 1, 5, 0, 3,
            ],
            ..BodyMesh::default()
        };
        assert!(mesh.curvature.is_none());
        mesh.ensure_curvature();
        let curvature = mesh.curvature.as_ref().unwrap();
        assert_eq!(curvature.len(), mesh.positions.len());
        assert!(!curvature.is_empty());
        assert!(curvature.iter().all(|value| value.is_finite()));
    }
}
