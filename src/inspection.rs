//! Read-only shape inspection calculations shared by commands and tests.

use std::sync::Arc;

use glam::DVec3;
use occt::Shape;

use crate::{
    assembly::posed_shape,
    document::{Body, BodyId},
};

/// Aggregated unit-density mass properties in internal millimetre units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AggregateProperties {
    pub volume: f64,
    pub area: f64,
    pub center: DVec3,
    pub inertia: [f64; 9],
    pub principal_inertia: [f64; 3],
}

/// One positive-volume pairwise intersection.
#[derive(Clone)]
pub struct Interference {
    pub first: BodyId,
    pub second: BodyId,
    pub first_name: String,
    pub second_name: String,
    pub volume: f64,
    pub shape: Arc<Shape>,
}

/// Aggregates selected bodies and shifts each tensor to the combined centroid.
pub fn aggregate_properties<'a>(
    bodies: impl IntoIterator<Item = &'a Body>,
) -> Result<AggregateProperties, String> {
    let mut parts = Vec::new();
    for body in bodies {
        let shape = posed_shape(&body.shape, body.pose)?;
        parts.push(
            shape
                .volume_properties()
                .map_err(|error| error.to_string())?,
        );
    }
    if parts.is_empty() {
        return Err(crate::i18n::t("No body selected").to_owned());
    }
    let volume: f64 = parts.iter().map(|part| part.volume).sum();
    let area = parts.iter().map(|part| part.area).sum();
    let center = if volume.abs() > f64::EPSILON {
        parts
            .iter()
            .map(|part| part.center_of_mass * part.volume)
            .sum::<DVec3>()
            / volume
    } else {
        DVec3::ZERO
    };
    let mut inertia = [0.0; 9];
    for part in &parts {
        for (sum, value) in inertia.iter_mut().zip(part.inertia_matrix) {
            *sum += value;
        }
        let d = part.center_of_mass - center;
        let d2 = d.length_squared();
        for row in 0..3 {
            for column in 0..3 {
                inertia[row * 3 + column] += part.volume * if row == column { d2 } else { 0.0 }
                    - part.volume * d[row] * d[column];
            }
        }
    }
    Ok(AggregateProperties {
        volume,
        area,
        center,
        principal_inertia: symmetric_eigenvalues(inertia),
        inertia,
    })
}

/// Pairwise positive-volume common shapes, preserving input order.
pub fn find_interferences<'a>(
    bodies: impl IntoIterator<Item = &'a Body>,
    limit: usize,
) -> Result<Vec<Interference>, String> {
    let bodies: Vec<_> = bodies.into_iter().collect();
    let posed = bodies
        .iter()
        .map(|body| posed_shape(&body.shape, body.pose))
        .collect::<Result<Vec<_>, _>>()?;
    let mut found = Vec::new();
    let mut tested = 0;
    for first in 0..bodies.len() {
        for second in first + 1..bodies.len() {
            if tested == limit {
                return Ok(found);
            }
            tested += 1;
            let shape = posed[first]
                .common(&posed[second])
                .map_err(|error| error.to_string())?;
            let volume = shape
                .volume_properties()
                .map_err(|error| error.to_string())?
                .volume
                .abs();
            if volume > 1.0e-9 {
                found.push(Interference {
                    first: bodies[first].id,
                    second: bodies[second].id,
                    first_name: bodies[first].name.clone(),
                    second_name: bodies[second].name.clone(),
                    volume,
                    shape: Arc::new(shape),
                });
            }
        }
    }
    Ok(found)
}

/// Closed-form eigenvalues of a real symmetric row-major 3×3 matrix.
pub fn symmetric_eigenvalues(matrix: [f64; 9]) -> [f64; 3] {
    let a = matrix[0];
    let b = (matrix[1] + matrix[3]) * 0.5;
    let c = (matrix[2] + matrix[6]) * 0.5;
    let d = matrix[4];
    let e = (matrix[5] + matrix[7]) * 0.5;
    let f = matrix[8];
    let p1 = b * b + c * c + e * e;
    let mut values = if p1 <= f64::EPSILON {
        [a, d, f]
    } else {
        let q = (a + d + f) / 3.0;
        let p = (((a - q).powi(2) + (d - q).powi(2) + (f - q).powi(2) + 2.0 * p1) / 6.0).sqrt();
        let inv_p = 1.0 / p;
        let aa = (a - q) * inv_p;
        let dd = (d - q) * inv_p;
        let ff = (f - q) * inv_p;
        let bb = b * inv_p;
        let cc = c * inv_p;
        let ee = e * inv_p;
        let determinant =
            aa * (dd * ff - ee * ee) - bb * (bb * ff - cc * ee) + cc * (bb * ee - cc * dd);
        let phi = (determinant * 0.5).clamp(-1.0, 1.0).acos() / 3.0;
        let largest = q + 2.0 * p * phi.cos();
        let smallest = q + 2.0 * p * (phi + std::f64::consts::TAU / 3.0).cos();
        [smallest, 3.0 * q - largest - smallest, largest]
    };
    values.sort_by(f64::total_cmp);
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Body, BodyKind, Material};

    fn body(id: u64, name: &str, min: DVec3, max: DVec3) -> Body {
        Body {
            id: BodyId(id),
            name: name.to_owned(),
            shape: Arc::new(Shape::box_from_corners(min, max).unwrap()),
            kind: BodyKind::Solid,
            visible: true,
            material: Material::default(),
            cosmetic_threads: Vec::new(),
            pose: glam::Mat4::IDENTITY,
        }
    }

    #[test]
    fn cuboid_principal_inertia_matches_analytic_values() {
        let item = body(1, "box", DVec3::ZERO, DVec3::new(2.0, 3.0, 4.0));
        let actual = aggregate_properties([&item]).unwrap().principal_inertia;
        let mass = 24.0;
        let mut expected = [
            mass * (3.0_f64.powi(2) + 4.0_f64.powi(2)) / 12.0,
            mass * (2.0_f64.powi(2) + 4.0_f64.powi(2)) / 12.0,
            mass * (2.0_f64.powi(2) + 3.0_f64.powi(2)) / 12.0,
        ];
        expected.sort_by(f64::total_cmp);
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1.0e-9);
        }
    }

    #[test]
    fn overlapping_and_disjoint_boxes_report_expected_interference() {
        let first = body(1, "A", DVec3::ZERO, DVec3::splat(2.0));
        let overlap = body(2, "B", DVec3::ONE, DVec3::splat(3.0));
        let disjoint = body(3, "C", DVec3::splat(4.0), DVec3::splat(5.0));
        let found = find_interferences([&first, &overlap], usize::MAX).unwrap();
        assert_eq!(found.len(), 1);
        assert!((found[0].volume - 1.0).abs() < 1.0e-9);
        assert!(
            find_interferences([&first, &disjoint], usize::MAX)
                .unwrap()
                .is_empty()
        );
    }
}
