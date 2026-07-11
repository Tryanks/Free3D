//! Hollow-shell geometry used by preview and commit.

use glam::DVec3;
use occt::Shape;

use crate::document::{BodyId, Document};

/// State captured for one shell thickness drag.
#[derive(Clone, Debug)]
pub struct ShellDrag {
    /// Body owning every selected face.
    pub body: BodyId,
    /// Faces removed to open the shell, in OCCT iteration order.
    pub face_indices: Vec<u32>,
    /// First selected face center.
    pub origin: DVec3,
    /// Inward arrow direction.
    pub direction: DVec3,
    /// Positive wall thickness.
    pub thickness: f64,
}

/// Builds a whole-body hollow result for preview.
pub fn preview(document: &Document, drag: &ShellDrag) -> Option<Shape> {
    let body = document.bodies.iter().find(|body| body.id == drag.body)?;
    if drag.face_indices.is_empty()
        || drag
            .face_indices
            .iter()
            .any(|&index| index as usize >= body.shape.face_count().ok().unwrap_or(0))
    {
        return None;
    }
    // Negative offset grows the inner wall into the solid while preserving
    // the original outer envelope.
    let shape = body
        .shape
        .hollow(&drag.face_indices, -drag.thickness)
        .ok()?;
    shape.mesh(0.5).ok()?;
    (shape.face_count().ok()? > 0).then_some(shape)
}
