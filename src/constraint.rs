//! Numeric constraints and a compact Levenberg--Marquardt sketch solver.

use std::f64::consts::PI;

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::sketch::{SketchEntity, SketchItem};

/// An entity index in a sketch's creation-ordered entity list.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityRef(pub usize);

/// A point belonging to an entity (`0` is line A or circle center, `1` is line B).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PointRef {
    /// Entity index in the sketch.
    pub entity: usize,
    /// Entity-local point index.
    pub point: u8,
}

/// A geometric or dimensional relationship between sketch entities.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum Constraint {
    /// Makes two points occupy the same coordinates.
    Coincident { a: PointRef, b: PointRef },
    /// Makes a line horizontal.
    Horizontal(EntityRef),
    /// Makes a line vertical.
    Vertical(EntityRef),
    /// Makes two line directions parallel.
    Parallel(EntityRef, EntityRef),
    /// Makes two line directions perpendicular.
    Perpendicular(EntityRef, EntityRef),
    /// Places both endpoints of the second line on the first line's infinite support.
    Collinear(EntityRef, EntityRef),
    /// Makes two line lengths or two circle radii equal.
    Equal(EntityRef, EntityRef),
    /// Places a point at a line's midpoint.
    Midpoint { point: PointRef, line: EntityRef },
    /// Makes two circle centers coincident.
    Concentric(EntityRef, EntityRef),
    /// Makes a line tangent to a circle or circular arc.
    Tangent { line: EntityRef, circle: EntityRef },
    /// Sets a line's length.
    Length {
        line: EntityRef,
        value: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Sets a circle's radius.
    Radius {
        circle: EntityRef,
        value: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Sets the distance between two points.
    Distance {
        a: PointRef,
        b: PointRef,
        value: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Sets the signed horizontal separation between two points.
    HDistance {
        a: PointRef,
        b: PointRef,
        value: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Sets the signed vertical separation between two points.
    VDistance {
        a: PointRef,
        b: PointRef,
        value: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Sets a circle's diameter.
    Diameter {
        circle: EntityRef,
        value: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Sets the directed angle from line A to line B.
    Angle {
        a: EntityRef,
        b: EntityRef,
        degrees: f64,
        #[serde(default)]
        expr: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        reference: bool,
    },
    /// Mirrors two points about a line axis.
    Symmetric {
        a: PointRef,
        b: PointRef,
        axis: EntityRef,
    },
    /// Places a point on a line, circle, or circular arc.
    PointOnObject { point: PointRef, target: EntityRef },
    /// Approximate G2 continuity using tangent and sampled second differences.
    G2 {
        spline: EntityRef,
        curve: EntityRef,
        spline_end: u8,
        curve_end: u8,
    },
}

impl Constraint {
    /// Returns the stored source expression for a dimensional constraint.
    pub fn expression(&self) -> Option<&str> {
        match self {
            Self::Length { expr, .. }
            | Self::Radius { expr, .. }
            | Self::Distance { expr, .. }
            | Self::HDistance { expr, .. }
            | Self::VDistance { expr, .. }
            | Self::Diameter { expr, .. }
            | Self::Angle { expr, .. } => expr.as_deref(),
            _ => None,
        }
    }

    /// Updates a dimensional constraint's numeric value and evaluation error.
    pub fn set_expression_result(&mut self, value: Option<f64>, error_message: Option<String>) {
        match self {
            Self::Length {
                value: target,
                error,
                ..
            }
            | Self::Radius {
                value: target,
                error,
                ..
            }
            | Self::Distance {
                value: target,
                error,
                ..
            }
            | Self::Diameter {
                value: target,
                error,
                ..
            } => {
                if let Some(value) = value {
                    *target = value.abs();
                }
                *error = error_message;
            }
            Self::HDistance {
                value: target,
                error,
                ..
            }
            | Self::VDistance {
                value: target,
                error,
                ..
            } => {
                if let Some(value) = value {
                    let sign = if target.is_sign_negative() { -1.0 } else { 1.0 };
                    *target = value.abs() * sign;
                }
                *error = error_message;
            }
            Self::Angle { degrees, error, .. } => {
                if let Some(value) = value {
                    *degrees = value;
                }
                *error = error_message;
            }
            _ => {}
        }
    }

    /// Sets the error flag of a dimensional constraint.
    pub fn set_expression_error(&mut self, message: Option<String>) {
        self.set_expression_result(None, message);
    }
    /// Returns whether this constraint refers to an entity index.
    pub fn references_entity(&self, entity: usize) -> bool {
        self.entity_indices().contains(&entity)
    }

    /// Remaps every entity reference through an old-to-new index table.
    pub fn remap_entities(&mut self, map: &[Option<usize>]) -> bool {
        let remap_entity = |entity: &mut EntityRef| match map.get(entity.0).copied().flatten() {
            Some(index) => {
                entity.0 = index;
                true
            }
            None => false,
        };
        let remap_point = |point: &mut PointRef| match map.get(point.entity).copied().flatten() {
            Some(index) => {
                point.entity = index;
                true
            }
            None => false,
        };
        match self {
            Self::Coincident { a, b }
            | Self::Distance { a, b, .. }
            | Self::HDistance { a, b, .. }
            | Self::VDistance { a, b, .. } => remap_point(a) && remap_point(b),
            Self::Horizontal(entity)
            | Self::Vertical(entity)
            | Self::Length { line: entity, .. }
            | Self::Radius { circle: entity, .. }
            | Self::Diameter { circle: entity, .. } => remap_entity(entity),
            Self::Parallel(a, b)
            | Self::Perpendicular(a, b)
            | Self::Collinear(a, b)
            | Self::Equal(a, b)
            | Self::Concentric(a, b)
            | Self::Angle { a, b, .. } => remap_entity(a) && remap_entity(b),
            Self::Midpoint { point, line } => remap_point(point) && remap_entity(line),
            Self::Tangent { line, circle } => remap_entity(line) && remap_entity(circle),
            Self::Symmetric { a, b, axis } => {
                remap_point(a) && remap_point(b) && remap_entity(axis)
            }
            Self::PointOnObject { point, target } => remap_point(point) && remap_entity(target),
            Self::G2 { spline, curve, .. } => remap_entity(spline) && remap_entity(curve),
        }
    }

    fn entity_indices(&self) -> Vec<usize> {
        match self {
            Self::Coincident { a, b }
            | Self::Distance { a, b, .. }
            | Self::HDistance { a, b, .. }
            | Self::VDistance { a, b, .. } => {
                vec![a.entity, b.entity]
            }
            Self::Horizontal(a)
            | Self::Vertical(a)
            | Self::Length { line: a, .. }
            | Self::Radius { circle: a, .. }
            | Self::Diameter { circle: a, .. } => vec![a.0],
            Self::Parallel(a, b)
            | Self::Perpendicular(a, b)
            | Self::Collinear(a, b)
            | Self::Equal(a, b)
            | Self::Concentric(a, b)
            | Self::Angle { a, b, .. } => vec![a.0, b.0],
            Self::Midpoint { point, line } => vec![point.entity, line.0],
            Self::Tangent { line, circle } => vec![line.0, circle.0],
            Self::Symmetric { a, b, axis } => vec![a.entity, b.entity, axis.0],
            Self::PointOnObject { point, target } => vec![point.entity, target.0],
            Self::G2 { spline, curve, .. } => vec![spline.0, curve.0],
        }
    }

    /// Returns whether this is a non-driving dimensional constraint.
    pub const fn is_reference(&self) -> bool {
        match self {
            Self::Length { reference, .. }
            | Self::Radius { reference, .. }
            | Self::Distance { reference, .. }
            | Self::HDistance { reference, .. }
            | Self::VDistance { reference, .. }
            | Self::Diameter { reference, .. }
            | Self::Angle { reference, .. } => *reference,
            _ => false,
        }
    }
}

/// Outcome of a constraint solve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolveResult {
    /// Whether all geometric residuals reached tolerance.
    pub converged: bool,
    /// Remaining independent degrees of freedom.
    pub dof: usize,
    /// Packed parameters classified as determined by pins or Jacobian pivots.
    pub determined: Vec<bool>,
}

/// Packs sketch entity coordinates into the solver parameter layout.
pub fn pack(entities: &[SketchEntity]) -> Vec<f64> {
    let mut values = Vec::new();
    for entity in entities {
        match entity {
            SketchEntity::Line { a, b } => values.extend([a.x, a.y, b.x, b.y]),
            SketchEntity::Circle { center, radius } => {
                values.extend([center.x, center.y, *radius]);
            }
            SketchEntity::Ellipse {
                center,
                major,
                minor_ratio,
            } => values.extend([center.x, center.y, major.x, major.y, *minor_ratio]),
            SketchEntity::Arc { start, end, mid } => {
                values.extend([start.x, start.y, end.x, end.y, mid.x, mid.y]);
            }
            SketchEntity::Spline { points } => {
                values.extend(points.iter().flat_map(|point| [point.x, point.y]));
            }
            SketchEntity::CvSpline { control, .. } => {
                values.extend(control.iter().flat_map(|point| [point.x, point.y]));
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => {
                values.extend([
                    center.x,
                    center.y,
                    major.x,
                    major.y,
                    *minor_ratio,
                    *start_angle,
                    *end_angle,
                ]);
            }
            SketchEntity::Point { at } => values.extend([at.x, at.y]),
        }
    }
    values
}

/// Unpacks solver parameters into sketch entities.
pub fn unpack(entities: &mut [SketchEntity], values: &[f64]) {
    let mut offset = 0;
    for entity in entities {
        match entity {
            SketchEntity::Line { a, b } => {
                *a = DVec2::new(values[offset], values[offset + 1]);
                *b = DVec2::new(values[offset + 2], values[offset + 3]);
                offset += 4;
            }
            SketchEntity::Circle { center, radius } => {
                *center = DVec2::new(values[offset], values[offset + 1]);
                *radius = values[offset + 2].abs();
                offset += 3;
            }
            SketchEntity::Ellipse {
                center,
                major,
                minor_ratio,
            } => {
                *center = DVec2::new(values[offset], values[offset + 1]);
                *major = DVec2::new(values[offset + 2], values[offset + 3]);
                *minor_ratio = values[offset + 4].abs();
                offset += 5;
            }
            SketchEntity::Arc { start, end, mid } => {
                *start = DVec2::new(values[offset], values[offset + 1]);
                *end = DVec2::new(values[offset + 2], values[offset + 3]);
                *mid = DVec2::new(values[offset + 4], values[offset + 5]);
                offset += 6;
            }
            SketchEntity::Spline { points } => {
                for point in points.iter_mut() {
                    *point = DVec2::new(values[offset], values[offset + 1]);
                    offset += 2;
                }
            }
            SketchEntity::CvSpline { control, .. } => {
                for point in control.iter_mut() {
                    *point = DVec2::new(values[offset], values[offset + 1]);
                    offset += 2;
                }
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => {
                *center = DVec2::new(values[offset], values[offset + 1]);
                *major = DVec2::new(values[offset + 2], values[offset + 3]);
                *minor_ratio = values[offset + 4].abs();
                *start_angle = values[offset + 5];
                *end_angle = values[offset + 6];
                offset += 7;
            }
            SketchEntity::Point { at } => {
                *at = DVec2::new(values[offset], values[offset + 1]);
                offset += 2;
            }
        }
    }
}

fn offsets(entities: &[SketchEntity]) -> Vec<usize> {
    let mut result = Vec::with_capacity(entities.len());
    let mut offset = 0;
    for entity in entities {
        result.push(offset);
        offset += match entity {
            SketchEntity::Line { .. } => 4,
            SketchEntity::Circle { .. } => 3,
            SketchEntity::Ellipse { .. } => 5,
            SketchEntity::Arc { .. } => 6,
            SketchEntity::Spline { points } => points.len() * 2,
            SketchEntity::CvSpline { control, .. } => control.len() * 2,
            SketchEntity::EllipseArc { .. } => 7,
            SketchEntity::Point { .. } => 2,
        };
    }
    result
}

fn point(values: &[f64], entities: &[SketchEntity], map: &[usize], reference: PointRef) -> DVec2 {
    let offset = map[reference.entity];
    match entities[reference.entity] {
        SketchEntity::Line { .. } if reference.point == 1 => {
            DVec2::new(values[offset + 2], values[offset + 3])
        }
        SketchEntity::Arc { .. } if reference.point == 1 => {
            DVec2::new(values[offset + 2], values[offset + 3])
        }
        SketchEntity::Spline { ref points } if reference.point == 1 => {
            let endpoint = offset + points.len().saturating_sub(1) * 2;
            DVec2::new(values[endpoint], values[endpoint + 1])
        }
        SketchEntity::CvSpline { ref control, .. } if reference.point == 1 => {
            let endpoint = offset + control.len().saturating_sub(1) * 2;
            DVec2::new(values[endpoint], values[endpoint + 1])
        }
        SketchEntity::EllipseArc { .. } => {
            let center = DVec2::new(values[offset], values[offset + 1]);
            let major = DVec2::new(values[offset + 2], values[offset + 3]);
            let minor = DVec2::new(-major.y, major.x) * values[offset + 4].abs();
            let angle = values[offset + if reference.point == 1 { 6 } else { 5 }];
            center + major * angle.cos() + minor * angle.sin()
        }
        _ => DVec2::new(values[offset], values[offset + 1]),
    }
}

fn line(
    values: &[f64],
    entities: &[SketchEntity],
    map: &[usize],
    entity: EntityRef,
) -> (DVec2, DVec2) {
    let offset = map[entity.0];
    match entities[entity.0] {
        SketchEntity::Line { .. } => (
            DVec2::new(values[offset], values[offset + 1]),
            DVec2::new(values[offset + 2], values[offset + 3]),
        ),
        SketchEntity::Circle { .. } => panic!("line constraint references a circle"),
        SketchEntity::Ellipse { .. } => panic!("line constraint references an ellipse"),
        SketchEntity::Arc { .. } => panic!("line constraint references an arc"),
        SketchEntity::Spline { .. } => panic!("line constraint references a spline"),
        SketchEntity::CvSpline { .. } => panic!("line constraint references a CV spline"),
        SketchEntity::EllipseArc { .. } => panic!("line constraint references an ellipse arc"),
        SketchEntity::Point { .. } => panic!("line constraint references a point"),
    }
}

fn circle(
    values: &[f64],
    entities: &[SketchEntity],
    map: &[usize],
    entity: EntityRef,
) -> (DVec2, f64) {
    let offset = map[entity.0];
    match entities[entity.0] {
        SketchEntity::Circle { .. } => (
            DVec2::new(values[offset], values[offset + 1]),
            values[offset + 2].abs(),
        ),
        SketchEntity::Arc { .. } => {
            let start = DVec2::new(values[offset], values[offset + 1]);
            let end = DVec2::new(values[offset + 2], values[offset + 3]);
            let mid = DVec2::new(values[offset + 4], values[offset + 5]);
            arc_circle(start, mid, end).expect("circular constraint references a collinear arc")
        }
        SketchEntity::Line { .. } => panic!("circle constraint references a line"),
        SketchEntity::Spline { .. } => panic!("circle constraint references a spline"),
        SketchEntity::CvSpline { .. } => panic!("circle constraint references a CV spline"),
        SketchEntity::Ellipse { .. } => panic!("circle constraint references an ellipse"),
        SketchEntity::EllipseArc { .. } => panic!("circle constraint references an ellipse arc"),
        SketchEntity::Point { .. } => panic!("circle constraint references a point"),
    }
}

fn arc_circle(start: DVec2, mid: DVec2, end: DVec2) -> Option<(DVec2, f64)> {
    let d =
        2.0 * (start.x * (mid.y - end.y) + mid.x * (end.y - start.y) + end.x * (start.y - mid.y));
    if d.abs() <= 1.0e-12 {
        return None;
    }
    let s = start.length_squared();
    let m = mid.length_squared();
    let e = end.length_squared();
    let center = DVec2::new(
        (s * (mid.y - end.y) + m * (end.y - start.y) + e * (start.y - mid.y)) / d,
        (s * (end.x - mid.x) + m * (start.x - end.x) + e * (mid.x - start.x)) / d,
    );
    Some((center, center.distance(start)))
}

fn residuals(values: &[f64], entities: &[SketchEntity], constraints: &[Constraint]) -> Vec<f64> {
    let map = offsets(entities);
    let mut residuals = Vec::new();
    for constraint in constraints {
        if constraint.is_reference() {
            continue;
        }
        match *constraint {
            Constraint::Coincident { a, b } => {
                let delta = point(values, entities, &map, a) - point(values, entities, &map, b);
                residuals.extend([delta.x, delta.y]);
            }
            Constraint::Horizontal(entity) => {
                let (a, b) = line(values, entities, &map, entity);
                residuals.push(b.y - a.y);
            }
            Constraint::Vertical(entity) => {
                let (a, b) = line(values, entities, &map, entity);
                residuals.push(b.x - a.x);
            }
            Constraint::Parallel(a, b) => {
                let (a0, a1) = line(values, entities, &map, a);
                let (b0, b1) = line(values, entities, &map, b);
                residuals.push((a1 - a0).perp_dot(b1 - b0));
            }
            Constraint::Perpendicular(a, b) => {
                let (a0, a1) = line(values, entities, &map, a);
                let (b0, b1) = line(values, entities, &map, b);
                residuals.push((a1 - a0).dot(b1 - b0));
            }
            Constraint::Collinear(a, b) => {
                let (a0, a1) = line(values, entities, &map, a);
                let (b0, b1) = line(values, entities, &map, b);
                let direction = a1 - a0;
                let scale = direction.length().max(1.0e-12);
                residuals.extend([
                    direction.perp_dot(b0 - a0) / scale,
                    direction.perp_dot(b1 - a0) / scale,
                ]);
            }
            Constraint::Equal(a, b) => {
                let measure = |entity: EntityRef| match entities[entity.0] {
                    SketchEntity::Line { .. } => {
                        let (start, end) = line(values, entities, &map, entity);
                        start.distance(end)
                    }
                    SketchEntity::Circle { .. } => circle(values, entities, &map, entity).1,
                    SketchEntity::Ellipse { .. } => {
                        panic!("equal constraint references an ellipse")
                    }
                    SketchEntity::Arc { .. } => panic!("equal constraint references an arc"),
                    SketchEntity::Spline { .. } => panic!("equal constraint references a spline"),
                    SketchEntity::CvSpline { .. } => {
                        panic!("equal constraint references a CV spline")
                    }
                    SketchEntity::EllipseArc { .. } => {
                        panic!("equal constraint references an ellipse arc")
                    }
                    SketchEntity::Point { .. } => panic!("equal constraint references a point"),
                };
                residuals.push(measure(a) - measure(b));
            }
            Constraint::Midpoint {
                point: p,
                line: entity,
            } => {
                let p = point(values, entities, &map, p);
                let (a, b) = line(values, entities, &map, entity);
                let delta = p - (a + b) * 0.5;
                residuals.extend([delta.x, delta.y]);
            }
            Constraint::Concentric(a, b) => {
                let delta =
                    circle(values, entities, &map, a).0 - circle(values, entities, &map, b).0;
                residuals.extend([delta.x, delta.y]);
            }
            Constraint::Tangent {
                line: entity,
                circle: circular,
            } => {
                let (a, b) = line(values, entities, &map, entity);
                let (center, radius) = circle(values, entities, &map, circular);
                let direction = b - a;
                let distance = direction.perp_dot(center - a) / direction.length().max(1.0e-12);
                residuals.push(distance.abs() - radius);
            }
            Constraint::Length {
                line: entity,
                value,
                ..
            } => {
                let (a, b) = line(values, entities, &map, entity);
                residuals.push(a.distance(b) - value);
            }
            Constraint::Radius {
                circle: entity,
                value,
                ..
            } => {
                residuals.push(circle(values, entities, &map, entity).1 - value);
            }
            Constraint::Distance { a, b, value, .. } => residuals.push(
                point(values, entities, &map, a).distance(point(values, entities, &map, b)) - value,
            ),
            Constraint::HDistance { a, b, value, .. } => residuals.push(
                point(values, entities, &map, a).x - point(values, entities, &map, b).x - value,
            ),
            Constraint::VDistance { a, b, value, .. } => residuals.push(
                point(values, entities, &map, a).y - point(values, entities, &map, b).y - value,
            ),
            Constraint::Diameter {
                circle: entity,
                value,
                ..
            } => residuals.push(circle(values, entities, &map, entity).1 * 2.0 - value),
            Constraint::Angle { a, b, degrees, .. } => {
                let (a0, a1) = line(values, entities, &map, a);
                let (b0, b1) = line(values, entities, &map, b);
                let a_direction = a1 - a0;
                let b_direction = b1 - b0;
                let actual =
                    b_direction.y.atan2(b_direction.x) - a_direction.y.atan2(a_direction.x);
                let target = degrees * PI / 180.0;
                residuals.push((actual - target).sin().atan2((actual - target).cos()));
            }
            Constraint::Symmetric { a, b, axis } => {
                let a = point(values, entities, &map, a);
                let b = point(values, entities, &map, b);
                let (axis_a, axis_b) = line(values, entities, &map, axis);
                let direction = axis_b - axis_a;
                let scale = direction.length().max(1.0e-12);
                residuals.push(direction.perp_dot((a + b) * 0.5 - axis_a) / scale);
                residuals.push((a - b).dot(direction) / scale);
            }
            Constraint::PointOnObject {
                point: point_ref,
                target,
            } => {
                let point = point(values, entities, &map, point_ref);
                match entities[target.0] {
                    SketchEntity::Line { .. } => {
                        let (a, b) = line(values, entities, &map, target);
                        residuals.push((b - a).perp_dot(point - a) / (b - a).length().max(1.0e-12));
                    }
                    SketchEntity::Circle { .. } | SketchEntity::Arc { .. } => {
                        let (center, radius) = circle(values, entities, &map, target);
                        residuals.push(point.distance(center) - radius);
                    }
                    _ => panic!("point-on-object target is not a line, circle, or arc"),
                }
            }
            Constraint::G2 {
                spline,
                curve,
                spline_end,
                curve_end,
            } => {
                let (sp, st, sc) = curve_neighborhood(values, entities, &map, spline, spline_end);
                let (cp, ct, cc) = curve_neighborhood(values, entities, &map, curve, curve_end);
                let delta = sp - cp;
                residuals.extend([
                    delta.x,
                    delta.y,
                    st.normalize_or_zero().perp_dot(ct.normalize_or_zero()),
                    sc - cc,
                ]);
            }
        }
    }
    residuals
}

/// Returns endpoint, tangent, and sampled curvature magnitude. Interpolating
/// splines deliberately use their editable-point neighborhood; this documents
/// and centralizes F5's approximate G2 model.
fn curve_neighborhood(
    values: &[f64],
    entities: &[SketchEntity],
    map: &[usize],
    entity: EntityRef,
    end: u8,
) -> (DVec2, DVec2, f64) {
    let offset = map[entity.0];
    let points: Vec<_> = match &entities[entity.0] {
        SketchEntity::Line { .. } => {
            let (a, b) = line(values, entities, map, entity);
            vec![a, (a + b) * 0.5, b]
        }
        SketchEntity::Arc { .. } => vec![
            DVec2::new(values[offset], values[offset + 1]),
            DVec2::new(values[offset + 4], values[offset + 5]),
            DVec2::new(values[offset + 2], values[offset + 3]),
        ],
        SketchEntity::Spline { points } => (0..points.len())
            .map(|index| DVec2::new(values[offset + index * 2], values[offset + index * 2 + 1]))
            .collect(),
        SketchEntity::CvSpline { control, .. } => (0..control.len())
            .map(|index| DVec2::new(values[offset + index * 2], values[offset + index * 2 + 1]))
            .collect(),
        _ => panic!("G2 constraint references an unsupported curve"),
    };
    let n = points.len();
    let sample = if end == 0 {
        [points[0], points[1.min(n - 1)], points[2.min(n - 1)]]
    } else {
        [
            points[n - 1],
            points[n.saturating_sub(2)],
            points[n.saturating_sub(3)],
        ]
    };
    let tangent = sample[1] - sample[0];
    let second = sample[2] - sample[1] * 2.0 + sample[0];
    let curvature = second.length() / tangent.length_squared().max(1.0e-12);
    (sample[0], tangent, curvature)
}

fn jacobian(
    values: &[f64],
    free: &[usize],
    entities: &[SketchEntity],
    constraints: &[Constraint],
) -> Vec<Vec<f64>> {
    let rows = residuals(values, entities, constraints).len();
    let mut result = vec![vec![0.0; free.len()]; rows];
    for (column, &parameter) in free.iter().enumerate() {
        let h = 1.0e-7 * values[parameter].abs().max(1.0);
        let mut plus = values.to_vec();
        let mut minus = values.to_vec();
        plus[parameter] += h;
        minus[parameter] -= h;
        let rp = residuals(&plus, entities, constraints);
        let rm = residuals(&minus, entities, constraints);
        for row in 0..rows {
            result[row][column] = (rp[row] - rm[row]) / (2.0 * h);
        }
    }
    result
}

fn solve_linear(mut matrix: Vec<Vec<f64>>, mut rhs: Vec<f64>) -> Option<Vec<f64>> {
    let n = rhs.len();
    for column in 0..n {
        let pivot = (column..n)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))?;
        if matrix[pivot][column].abs() < 1.0e-15 {
            return None;
        }
        matrix.swap(column, pivot);
        rhs.swap(column, pivot);
        for row in column + 1..n {
            let factor = matrix[row][column] / matrix[column][column];
            for index in column..n {
                matrix[row][index] -= factor * matrix[column][index];
            }
            rhs[row] -= factor * rhs[column];
        }
    }
    let mut result = vec![0.0; n];
    for row in (0..n).rev() {
        let tail: f64 = (row + 1..n)
            .map(|column| matrix[row][column] * result[column])
            .sum();
        result[row] = (rhs[row] - tail) / matrix[row][row];
    }
    Some(result)
}

fn pivot_columns(mut matrix: Vec<Vec<f64>>) -> Vec<usize> {
    if matrix.is_empty() || matrix[0].is_empty() {
        return Vec::new();
    }
    let rows = matrix.len();
    let columns = matrix[0].len();
    let scale = matrix
        .iter()
        .flatten()
        .map(|value| value.abs())
        .fold(0.0, f64::max);
    let tolerance = scale.max(1.0) * 1.0e-8;
    let mut pivot_row = 0;
    let mut pivots = Vec::new();
    for column in 0..columns {
        if pivot_row == rows {
            break;
        }
        let Some(row) = (pivot_row..rows)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))
        else {
            break;
        };
        if matrix[row][column].abs() <= tolerance {
            continue;
        }
        matrix.swap(pivot_row, row);
        let pivot_values = matrix[pivot_row][column..].to_vec();
        for row in pivot_row + 1..rows {
            let factor = matrix[row][column] / matrix[pivot_row][column];
            for (value, pivot) in matrix[row][column..].iter_mut().zip(&pivot_values) {
                *value -= factor * pivot;
            }
        }
        pivots.push(column);
        pivot_row += 1;
    }
    pivots
}

/// Solves constraints while holding the listed packed parameter indices fixed.
pub fn solve(
    entities: &mut [SketchEntity],
    constraints: &[Constraint],
    pinned: &[usize],
) -> SolveResult {
    let start = pack(entities);
    let mut values = start.clone();
    let free: Vec<_> = (0..values.len())
        .filter(|index| !pinned.contains(index))
        .collect();
    let mut lambda = 1.0e-3;
    const MU: f64 = 1.0e-6;

    for _ in 0..60 {
        let geometric = residuals(&values, entities, constraints);
        if geometric.iter().all(|value| value.abs() < 1.0e-9) {
            break;
        }
        let jacobian = jacobian(&values, &free, entities, constraints);
        let n = free.len();
        let mut normal = vec![vec![0.0; n]; n];
        let mut gradient = vec![0.0; n];
        for row in 0..geometric.len() {
            for a in 0..n {
                gradient[a] += jacobian[row][a] * geometric[row];
                for b in 0..n {
                    normal[a][b] += jacobian[row][a] * jacobian[row][b];
                }
            }
        }
        for (local, &parameter) in free.iter().enumerate() {
            normal[local][local] += MU * MU + lambda;
            gradient[local] += MU * MU * (values[parameter] - start[parameter]);
        }
        let Some(step) = solve_linear(normal, gradient.iter().map(|value| -value).collect()) else {
            lambda *= 10.0;
            continue;
        };
        let current_cost: f64 = geometric.iter().map(|value| value * value).sum();
        let mut candidate = values.clone();
        for (local, &parameter) in free.iter().enumerate() {
            candidate[parameter] += step[local];
        }
        let next = residuals(&candidate, entities, constraints);
        let next_cost: f64 = next.iter().map(|value| value * value).sum();
        if next_cost < current_cost {
            values = candidate;
            lambda = (lambda * 0.3).max(1.0e-12);
        } else {
            lambda *= 10.0;
        }
    }

    unpack(entities, &values);
    let geometric = residuals(&values, entities, constraints);
    let converged = geometric.iter().all(|value| value.abs() < 1.0e-9);
    let pivots = pivot_columns(jacobian(&values, &free, entities, constraints));
    let mut determined = vec![false; values.len()];
    for &parameter in pinned {
        if let Some(flag) = determined.get_mut(parameter) {
            *flag = true;
        }
    }
    for &local in &pivots {
        determined[free[local]] = true;
    }
    SolveResult {
        converged,
        dof: free.len().saturating_sub(pivots.len()),
        determined,
    }
}

/// Solves the geometry stored inside sketch items without changing construction flags.
pub fn solve_items(
    items: &mut [SketchItem],
    constraints: &[Constraint],
    pinned: &[usize],
) -> SolveResult {
    let mut entities: Vec<_> = items.iter().map(|item| item.geo.clone()).collect();
    let result = solve(&mut entities, constraints, pinned);
    for (item, geo) in items.iter_mut().zip(entities) {
        item.geo = geo;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: (f64, f64), b: (f64, f64)) -> SketchEntity {
        SketchEntity::Line {
            a: a.into(),
            b: b.into(),
        }
    }

    fn rectangle_constraints() -> Vec<Constraint> {
        let mut constraints = (0..4)
            .map(|index| Constraint::Coincident {
                a: PointRef {
                    entity: index,
                    point: 1,
                },
                b: PointRef {
                    entity: (index + 1) % 4,
                    point: 0,
                },
            })
            .collect::<Vec<_>>();
        constraints.extend([
            Constraint::Horizontal(EntityRef(0)),
            Constraint::Vertical(EntityRef(1)),
            Constraint::Horizontal(EntityRef(2)),
            Constraint::Vertical(EntityRef(3)),
            Constraint::Length {
                line: EntityRef(0),
                value: 40.0,
                expr: None,
                error: None,
                reference: false,
            },
            Constraint::Distance {
                a: PointRef {
                    entity: 0,
                    point: 0,
                },
                b: PointRef {
                    entity: 3,
                    point: 0,
                },
                value: 30.0,
                expr: None,
                error: None,
                reference: false,
            },
        ]);
        constraints
    }

    #[test]
    fn rectangle_solves_and_pins_remove_rigid_translation() {
        let initial = vec![
            line((0.0, 0.0), (39.0, 1.0)),
            line((40.0, 0.0), (41.0, 29.0)),
            line((40.0, 30.0), (1.0, 31.0)),
            line((0.0, 30.0), (-1.0, 1.0)),
        ];
        let constraints = rectangle_constraints();
        let mut floating = initial.clone();
        let result = solve(&mut floating, &constraints, &[]);
        assert!(result.converged);
        assert_eq!(result.dof, 2);

        let mut fixed = initial;
        let result = solve(&mut fixed, &constraints, &[0, 1]);
        assert!(result.converged);
        assert_eq!(result.dof, 0);
        let SketchEntity::Line { a, b } = fixed[0] else {
            unreachable!()
        };
        assert!(a.distance(DVec2::ZERO) < 1.0e-12);
        assert!((a.distance(b) - 40.0).abs() < 1.0e-9);
        let SketchEntity::Line { a: top, .. } = fixed[2] else {
            unreachable!()
        };
        assert!((top.y.abs() - 30.0).abs() < 1.0e-9);
    }

    #[test]
    fn tangent_line_circle_solves() {
        let mut entities = vec![
            line((-5.0, 0.3), (5.0, -0.2)),
            SketchEntity::Circle {
                center: DVec2::new(0.0, 2.8),
                radius: 2.0,
            },
        ];
        let result = solve(
            &mut entities,
            &[
                Constraint::Horizontal(EntityRef(0)),
                Constraint::Tangent {
                    line: EntityRef(0),
                    circle: EntityRef(1),
                },
            ],
            &[0, 2, 4, 6],
        );
        assert!(result.converged);
        let SketchEntity::Line { a, .. } = entities[0] else {
            unreachable!()
        };
        let SketchEntity::Circle { center, radius } = entities[1] else {
            unreachable!()
        };
        assert!(((center.y - a.y).abs() - radius).abs() < 1.0e-9);
    }

    #[test]
    fn perpendicular_pair_solves() {
        let mut entities = vec![line((0.0, 0.0), (4.0, 0.0)), line((0.0, 0.0), (1.0, 3.0))];
        let result = solve(
            &mut entities,
            &[Constraint::Perpendicular(EntityRef(0), EntityRef(1))],
            &[0, 1, 2, 3, 4, 5],
        );
        assert!(result.converged);
        let (a0, a1) = line_points(&entities[0]);
        let (b0, b1) = line_points(&entities[1]);
        assert!((a1 - a0).dot(b1 - b0).abs() < 1.0e-9);
    }

    #[test]
    fn collinear_pair_solves_both_endpoint_distances() {
        let mut entities = vec![line((0.0, 0.0), (10.0, 0.0)), line((2.0, 2.0), (8.0, 3.0))];
        let result = solve(
            &mut entities,
            &[Constraint::Collinear(EntityRef(0), EntityRef(1))],
            &[0, 1, 2, 3, 4, 6],
        );
        assert!(result.converged);
        let (_, b) = line_points(&entities[1]);
        let (a, _) = line_points(&entities[1]);
        assert!(a.y.abs() < 1.0e-9 && b.y.abs() < 1.0e-9);
    }

    #[test]
    fn cv_spline_pack_unpack_roundtrips_control_points() {
        let original = SketchEntity::CvSpline {
            control: vec![DVec2::ZERO, DVec2::X, DVec2::Y, DVec2::ONE],
            degree: 3,
        };
        let values = pack(std::slice::from_ref(&original));
        let mut unpacked = [SketchEntity::CvSpline {
            control: vec![DVec2::ZERO; 4],
            degree: 3,
        }];
        unpack(&mut unpacked, &values);
        assert_eq!(unpacked[0], original);
    }

    fn line_points(entity: &SketchEntity) -> (DVec2, DVec2) {
        match entity {
            SketchEntity::Line { a, b } => (*a, *b),
            _ => unreachable!(),
        }
    }

    #[test]
    fn contradictory_constraints_do_not_converge() {
        let mut entities = vec![line((0.0, 0.0), (2.0, 1.0))];
        let result = solve(
            &mut entities,
            &[
                Constraint::Horizontal(EntityRef(0)),
                Constraint::Vertical(EntityRef(0)),
                Constraint::Length {
                    line: EntityRef(0),
                    value: 10.0,
                    expr: None,
                    error: None,
                    reference: false,
                },
            ],
            &[0, 1],
        );
        assert!(!result.converged);
    }

    #[test]
    fn arc_pack_unpack_roundtrips_six_parameters() {
        let original = SketchEntity::Arc {
            start: DVec2::new(1.0, 2.0),
            end: DVec2::new(3.0, 4.0),
            mid: DVec2::new(5.0, 6.0),
        };
        let values = pack(std::slice::from_ref(&original));
        assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut unpacked = [SketchEntity::Arc {
            start: DVec2::ZERO,
            end: DVec2::ZERO,
            mid: DVec2::ZERO,
        }];
        unpack(&mut unpacked, &values);
        assert_eq!(unpacked[0], original);
    }

    #[test]
    fn spline_pack_unpack_roundtrips_all_fit_points() {
        let original = SketchEntity::Spline {
            points: vec![
                DVec2::new(1.0, 2.0),
                DVec2::new(3.0, 4.0),
                DVec2::new(5.0, 6.0),
            ],
        };
        let values = pack(std::slice::from_ref(&original));
        assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut unpacked = [SketchEntity::Spline {
            points: vec![DVec2::ZERO; 3],
        }];
        unpack(&mut unpacked, &values);
        assert_eq!(unpacked[0], original);
    }

    #[test]
    fn ellipse_pack_unpack_roundtrips_five_parameters() {
        let original = SketchEntity::Ellipse {
            center: DVec2::new(2.0, 3.0),
            major: DVec2::new(8.0, 4.0),
            minor_ratio: 0.35,
        };
        let values = pack(std::slice::from_ref(&original));
        assert_eq!(values.len(), 5);
        let mut unpacked = [SketchEntity::Ellipse {
            center: DVec2::ZERO,
            major: DVec2::ZERO,
            minor_ratio: 0.0,
        }];
        unpack(&mut unpacked, &values);
        assert_eq!(unpacked[0], original);
    }

    #[test]
    fn point_and_ellipse_arc_parameter_layouts_roundtrip() {
        let original = [
            SketchEntity::Point {
                at: DVec2::new(1.0, 2.0),
            },
            SketchEntity::EllipseArc {
                center: DVec2::new(3.0, 4.0),
                major: DVec2::new(8.0, 1.0),
                minor_ratio: 0.4,
                start_angle: 0.2,
                end_angle: 2.7,
            },
        ];
        let values = pack(&original);
        assert_eq!(values.len(), 9);
        let mut unpacked = [
            SketchEntity::Point { at: DVec2::ZERO },
            SketchEntity::EllipseArc {
                center: DVec2::ZERO,
                major: DVec2::ZERO,
                minor_ratio: 0.0,
                start_angle: 0.0,
                end_angle: 0.0,
            },
        ];
        unpack(&mut unpacked, &values);
        assert_eq!(unpacked, original);
    }

    #[test]
    fn symmetric_solves_to_mirrored_positions() {
        let mut entities = vec![
            SketchEntity::Point {
                at: DVec2::new(3.0, 2.0),
            },
            SketchEntity::Point {
                at: DVec2::new(-1.0, 1.0),
            },
            line((0.0, -10.0), (0.0, 10.0)),
        ];
        let result = solve(
            &mut entities,
            &[Constraint::Symmetric {
                a: PointRef {
                    entity: 0,
                    point: 0,
                },
                b: PointRef {
                    entity: 1,
                    point: 0,
                },
                axis: EntityRef(2),
            }],
            &[0, 1, 4, 5, 6, 7],
        );
        assert!(result.converged);
        let SketchEntity::Point { at } = entities[1] else {
            unreachable!()
        };
        assert!(at.distance(DVec2::new(-3.0, 2.0)) < 1.0e-8);
    }

    #[test]
    fn point_on_circle_converges() {
        let mut entities = vec![
            SketchEntity::Point {
                at: DVec2::new(3.0, 2.0),
            },
            SketchEntity::Circle {
                center: DVec2::ZERO,
                radius: 5.0,
            },
        ];
        let result = solve(
            &mut entities,
            &[Constraint::PointOnObject {
                point: PointRef {
                    entity: 0,
                    point: 0,
                },
                target: EntityRef(1),
            }],
            &[0, 2, 3, 4],
        );
        assert!(result.converged);
        let SketchEntity::Point { at } = entities[0] else {
            unreachable!()
        };
        assert!((at.length() - 5.0).abs() < 1.0e-9);
    }

    #[test]
    fn horizontal_and_vertical_distances_drive_signed_separation() {
        let mut entities = vec![
            SketchEntity::Point { at: DVec2::ZERO },
            SketchEntity::Point {
                at: DVec2::new(2.0, 3.0),
            },
        ];
        let result = solve(
            &mut entities,
            &[
                Constraint::HDistance {
                    a: PointRef {
                        entity: 0,
                        point: 0,
                    },
                    b: PointRef {
                        entity: 1,
                        point: 0,
                    },
                    value: -10.0,
                    expr: None,
                    error: None,
                    reference: false,
                },
                Constraint::VDistance {
                    a: PointRef {
                        entity: 0,
                        point: 0,
                    },
                    b: PointRef {
                        entity: 1,
                        point: 0,
                    },
                    value: 7.0,
                    expr: None,
                    error: None,
                    reference: false,
                },
            ],
            &[0, 1],
        );
        assert!(result.converged);
        let SketchEntity::Point { at } = entities[1] else {
            unreachable!()
        };
        assert!(at.distance(DVec2::new(10.0, -7.0)) < 1.0e-8);
    }

    #[test]
    fn diameter_is_twice_radius() {
        let mut entities = vec![SketchEntity::Circle {
            center: DVec2::ZERO,
            radius: 2.0,
        }];
        let result = solve(
            &mut entities,
            &[Constraint::Diameter {
                circle: EntityRef(0),
                value: 12.0,
                expr: None,
                error: None,
                reference: false,
            }],
            &[0, 1],
        );
        assert!(result.converged);
        let SketchEntity::Circle { radius, .. } = entities[0] else {
            unreachable!()
        };
        assert!((radius - 6.0).abs() < 1.0e-9);
    }

    #[test]
    fn reference_dimension_measures_without_moving_geometry() {
        let original = line((1.0, 2.0), (4.0, 6.0));
        let mut entities = vec![original.clone()];
        let result = solve(
            &mut entities,
            &[Constraint::Length {
                line: EntityRef(0),
                value: 100.0,
                expr: None,
                error: None,
                reference: true,
            }],
            &[],
        );
        assert!(result.converged);
        assert_eq!(entities[0], original);
        let (a, b) = line_points(&entities[0]);
        assert!((a.distance(b) - 5.0).abs() < 1.0e-12);
    }

    #[test]
    fn pivot_classification_distinguishes_fixed_and_dangling_lines() {
        let mut entities = vec![
            line((0.0, 0.0), (3.0, 0.2)),
            line((10.0, 10.0), (12.0, 13.0)),
        ];
        let result = solve(
            &mut entities,
            &[
                Constraint::Horizontal(EntityRef(0)),
                Constraint::Length {
                    line: EntityRef(0),
                    value: 4.0,
                    expr: None,
                    error: None,
                    reference: false,
                },
            ],
            &[0, 1],
        );
        assert!(result.converged);
        assert!(result.determined[..4].iter().all(|value| *value));
        assert!(result.determined[4..].iter().all(|value| !*value));
    }

    #[test]
    fn dimensional_reference_flag_is_backward_compatible_in_serde() {
        let legacy = r#"{"t":"Length","v":{"line":0,"value":5.0}}"#;
        let constraint: Constraint = serde_json::from_str(legacy).expect("legacy dimension");
        assert_eq!(
            constraint,
            Constraint::Length {
                line: EntityRef(0),
                value: 5.0,
                expr: None,
                error: None,
                reference: false
            }
        );
        let reference = Constraint::Diameter {
            circle: EntityRef(2),
            value: 12.0,
            expr: None,
            error: None,
            reference: true,
        };
        let json = serde_json::to_string(&reference).expect("serialize reference dimension");
        assert_eq!(
            serde_json::from_str::<Constraint>(&json).expect("deserialize reference dimension"),
            reference
        );
    }
}
