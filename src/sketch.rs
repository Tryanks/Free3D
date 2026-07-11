//! Plane-local sketch geometry and closed-profile construction.

use glam::{DVec2, DVec3};
use occt::{Edge, Face, Wire};
use serde::{Deserialize, Deserializer, Serialize};

use crate::constraint::{Constraint, EntityRef, PointRef};
use crate::document::BodyId;
use crate::tools::extrude::face_frame;

const ENDPOINT_EPSILON: f64 = 1.0e-6;

/// Stable identifier for a document sketch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct SketchId(pub u64);

/// An orthonormal 2D coordinate system embedded in model space.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct SketchPlane {
    /// Plane-local origin in world coordinates.
    pub origin: DVec3,
    /// Plane-local positive X direction.
    pub x_axis: DVec3,
    /// Plane-local positive Y direction.
    pub y_axis: DVec3,
}

impl SketchPlane {
    /// Creates the world XY plane.
    pub const fn xy() -> Self {
        Self {
            origin: DVec3::ZERO,
            x_axis: DVec3::X,
            y_axis: DVec3::Y,
        }
    }

    /// Creates a sketch plane from a planar BRep face.
    pub fn from_face(shape: &occt::Shape, face_index: u32) -> Option<Self> {
        let (origin, normal) = face_frame(shape, face_index)?;
        let reference = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
            DVec3::X
        } else if normal.y.abs() <= normal.z.abs() {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let x_axis = reference.cross(normal).normalize_or_zero();
        let y_axis = normal.cross(x_axis).normalize_or_zero();
        (x_axis.length_squared() > 0.99 && y_axis.length_squared() > 0.99).then_some(Self {
            origin,
            x_axis,
            y_axis,
        })
    }

    /// Returns the right-handed unit normal.
    pub fn normal(self) -> DVec3 {
        self.x_axis.cross(self.y_axis).normalize_or_zero()
    }

    /// Maps plane-local coordinates into model space.
    pub fn to_world(self, point: DVec2) -> DVec3 {
        self.origin + self.x_axis * point.x + self.y_axis * point.y
    }

    /// Projects a model-space point into plane-local coordinates.
    pub fn to_local(self, point: DVec3) -> DVec2 {
        let delta = point - self.origin;
        DVec2::new(delta.dot(self.x_axis), delta.dot(self.y_axis))
    }
}

/// One committed sketch curve in plane-local coordinates.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum SketchEntity {
    /// Finite straight segment.
    Line { a: DVec2, b: DVec2 },
    /// Full circle.
    Circle { center: DVec2, radius: f64 },
    /// Full ellipse with a semi-major vector and minor/major radius ratio.
    Ellipse {
        center: DVec2,
        major: DVec2,
        minor_ratio: f64,
    },
    /// Circular arc passing through start, mid, and end in that order.
    Arc {
        start: DVec2,
        end: DVec2,
        mid: DVec2,
    },
    /// Interpolating B-spline through creation-ordered fit points.
    Spline { points: Vec<DVec2> },
    /// Clamped uniform B-spline defined directly by control points.
    CvSpline { control: Vec<DVec2>, degree: u8 },
    /// Partial ellipse parameterized in the ellipse's local angular coordinates.
    EllipseArc {
        center: DVec2,
        major: DVec2,
        minor_ratio: f64,
        start_angle: f64,
        end_angle: f64,
    },
    /// A standalone reference point.
    Point { at: DVec2 },
}

/// Geometry plus its profile-participation state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SketchItem {
    /// Editable sketch geometry.
    pub geo: SketchEntity,
    /// Construction geometry remains visible and constrainable but makes no profiles.
    pub construction: bool,
}

impl SketchItem {
    /// Wraps ordinary profile-capable geometry.
    pub const fn regular(geo: SketchEntity) -> Self {
        Self {
            geo,
            construction: false,
        }
    }

    /// Wraps reference-only construction geometry.
    pub const fn construction(geo: SketchEntity) -> Self {
        Self {
            geo,
            construction: true,
        }
    }
}

impl From<SketchEntity> for SketchItem {
    fn from(geo: SketchEntity) -> Self {
        Self::regular(geo)
    }
}

impl<'de> Deserialize<'de> for SketchItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Compatible {
            Item {
                geo: SketchEntity,
                #[serde(default)]
                construction: bool,
            },
            Legacy(SketchEntity),
        }
        Ok(match Compatible::deserialize(deserializer)? {
            Compatible::Item { geo, construction } => Self { geo, construction },
            Compatible::Legacy(geo) => Self::regular(geo),
        })
    }
}

/// One detected closed region boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum Profile {
    /// Ordered polygon vertices; the closing edge is implicit.
    LineLoop(Vec<DVec2>),
    /// Circular boundary.
    Circle { center: DVec2, radius: f64 },
    /// Elliptical boundary.
    Ellipse {
        center: DVec2,
        major: DVec2,
        minor_ratio: f64,
    },
    /// Ordered line/arc entity indices and whether each is traversed backwards.
    CurveLoop(Vec<(usize, bool)>),
}

/// A document sketch and its committed entities.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sketch {
    /// Stable identifier.
    pub id: SketchId,
    /// Embedding plane.
    pub plane: SketchPlane,
    /// Curves in creation order.
    pub entities: Vec<SketchItem>,
    /// Geometric and dimensional relationships between entities.
    pub constraints: Vec<Constraint>,
    /// Packed parameter indices held fixed by the solver.
    pub pinned: Vec<usize>,
    /// Defined-state cache parallel to `entities`.
    #[serde(skip)]
    pub defined: Vec<bool>,
    /// Items-panel visibility.
    pub visible: bool,
    /// Body whose planar face supplied this sketch plane, when applicable.
    pub support_body: Option<BodyId>,
    /// OCCT-derived display/picking polylines parallel to `entities`.
    #[serde(skip)]
    pub spline_samples: Vec<Option<Vec<DVec2>>>,
}

impl Sketch {
    /// Creates an empty visible sketch.
    pub fn new(id: SketchId, plane: SketchPlane) -> Self {
        Self {
            id,
            plane,
            entities: Vec::new(),
            constraints: Vec::new(),
            pinned: Vec::new(),
            defined: Vec::new(),
            visible: true,
            support_body: None,
            spline_samples: Vec::new(),
        }
    }

    /// Rebuilds OCCT-derived spline polylines after an entity edit.
    pub fn refresh_spline_samples(&mut self) {
        self.spline_samples = self
            .entities
            .iter()
            .map(|entity| match &entity.geo {
                SketchEntity::Spline { points } => Some(sample_spline(points, self.plane)),
                SketchEntity::CvSpline { control, degree } => {
                    Some(sample_cv_spline(control, *degree, self.plane))
                }
                _ => None,
            })
            .collect();
    }

    /// Returns a cached OCCT spline polyline, falling back for directly-built sketches.
    pub fn spline_polyline(&self, entity: usize, points: &[DVec2]) -> Vec<DVec2> {
        self.spline_samples
            .get(entity)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| sample_spline(points, self.plane))
    }

    /// Finds simple closed line chains and treats every circle as a profile.
    pub fn profiles(&self) -> Vec<Profile> {
        let curves: Vec<(usize, DVec2, DVec2)> = self
            .entities
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.construction)
            .filter_map(|(index, entity)| match &entity.geo {
                SketchEntity::Line { a, b } => Some((index, *a, *b)),
                SketchEntity::Arc { start, end, .. } => Some((index, *start, *end)),
                SketchEntity::Spline { points } if points.len() >= 2 => {
                    Some((index, points[0], *points.last()?))
                }
                SketchEntity::CvSpline { control, .. } if control.len() >= 2 => {
                    Some((index, control[0], *control.last()?))
                }
                SketchEntity::EllipseArc {
                    center,
                    major,
                    minor_ratio,
                    start_angle,
                    end_angle,
                } => Some((
                    index,
                    ellipse_point(*center, *major, *minor_ratio, *start_angle),
                    ellipse_point(*center, *major, *minor_ratio, *end_angle),
                )),
                SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => None,
                SketchEntity::Spline { .. }
                | SketchEntity::CvSpline { .. }
                | SketchEntity::Point { .. } => None,
            })
            .collect();
        let mut consumed = vec![false; curves.len()];
        let mut profiles = Vec::new();

        for start_index in 0..curves.len() {
            if consumed[start_index] {
                continue;
            }
            let (_, start, mut cursor) = curves[start_index];
            let mut local = vec![start_index];
            let mut vertices = vec![start, cursor];
            let mut ordered = vec![(curves[start_index].0, false)];
            while !near(cursor, start) {
                let next = curves
                    .iter()
                    .enumerate()
                    .find_map(|(index, (entity, a, b))| {
                        (!consumed[index] && !local.contains(&index)).then(|| {
                            if near(*a, cursor) {
                                Some((index, *entity, *b, false))
                            } else if near(*b, cursor) {
                                Some((index, *entity, *a, true))
                            } else {
                                None
                            }
                        })?
                    });
                let Some((index, entity, endpoint, reversed)) = next else {
                    break;
                };
                local.push(index);
                ordered.push((entity, reversed));
                cursor = endpoint;
                vertices.push(cursor);
            }
            if local.len() >= 2 && near(cursor, start) {
                vertices.pop();
                for index in local {
                    consumed[index] = true;
                }
                if ordered.iter().all(|(index, _)| {
                    matches!(self.entities[*index].geo, SketchEntity::Line { .. })
                }) {
                    profiles.push(Profile::LineLoop(vertices));
                } else {
                    profiles.push(Profile::CurveLoop(ordered));
                }
            }
        }

        profiles.extend(
            self.entities
                .iter()
                .filter(|item| !item.construction)
                .filter_map(|entity| match &entity.geo {
                    SketchEntity::Circle { center, radius } if *radius > ENDPOINT_EPSILON => {
                        Some(Profile::Circle {
                            center: *center,
                            radius: *radius,
                        })
                    }
                    SketchEntity::Ellipse {
                        center,
                        major,
                        minor_ratio,
                    } if major.length() > ENDPOINT_EPSILON && *minor_ratio > ENDPOINT_EPSILON => {
                        Some(Profile::Ellipse {
                            center: *center,
                            major: *major,
                            minor_ratio: *minor_ratio,
                        })
                    }
                    _ => None,
                }),
        );
        profiles
    }

    /// Returns connected, non-branching open line chains as ordered entity indices.
    ///
    /// Closed line loops are profiles and are deliberately excluded. Circles are
    /// also excluded because they cannot form an open trajectory.
    pub fn open_chains(&self) -> Vec<Vec<usize>> {
        let curve_indices: Vec<_> = self
            .entities
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.construction)
            .filter_map(|(index, entity)| {
                matches!(
                    &entity.geo,
                    SketchEntity::Line { .. }
                        | SketchEntity::Arc { .. }
                        | SketchEntity::EllipseArc { .. }
                        | SketchEntity::Spline { .. }
                        | SketchEntity::CvSpline { .. }
                )
                .then_some(index)
            })
            .collect();
        let mut consumed = vec![false; self.entities.len()];
        let mut chains = Vec::new();

        for &seed in &curve_indices {
            if consumed[seed] {
                continue;
            }
            let mut component = vec![seed];
            let mut cursor = 0;
            while cursor < component.len() {
                let current = component[cursor];
                let (a, b) = self
                    .entity_endpoints(current)
                    .expect("curve index was filtered above");
                for &candidate in &curve_indices {
                    if component.contains(&candidate) {
                        continue;
                    }
                    let (c, d) = self
                        .entity_endpoints(candidate)
                        .expect("curve index was filtered above");
                    if near(a, c) || near(a, d) || near(b, c) || near(b, d) {
                        component.push(candidate);
                    }
                }
                cursor += 1;
            }
            for &index in &component {
                consumed[index] = true;
            }
            if let Some(ordered) = self.order_open_chain(&component) {
                chains.push(ordered);
            }
        }
        chains
    }

    fn order_open_chain(&self, entity_indices: &[usize]) -> Option<Vec<usize>> {
        if entity_indices.is_empty() {
            return None;
        }
        let endpoints = |index: usize| match &self.entities.get(index)?.geo {
            SketchEntity::Line { a, b } => Some((*a, *b)),
            SketchEntity::Arc { start, end, .. } => Some((*start, *end)),
            SketchEntity::Spline { points } if points.len() >= 2 => {
                Some((points[0], *points.last()?))
            }
            SketchEntity::CvSpline { control, .. } if control.len() >= 2 => {
                Some((control[0], *control.last()?))
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => Some((
                ellipse_point(*center, *major, *minor_ratio, *start_angle),
                ellipse_point(*center, *major, *minor_ratio, *end_angle),
            )),
            SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => None,
            SketchEntity::Spline { .. }
            | SketchEntity::CvSpline { .. }
            | SketchEntity::Point { .. } => None,
        };
        let degree = |point: DVec2| {
            entity_indices
                .iter()
                .filter_map(|&index| endpoints(index))
                .filter(|(a, b)| near(*a, point) || near(*b, point))
                .count()
        };
        let start = entity_indices.iter().find_map(|&index| {
            let (a, b) = endpoints(index)?;
            (degree(a) == 1)
                .then_some((index, a))
                .or_else(|| (degree(b) == 1).then_some((index, b)))
        })?;
        if entity_indices
            .iter()
            .filter_map(|&index| endpoints(index))
            .any(|(a, b)| !matches!(degree(a), 1 | 2) || !matches!(degree(b), 1 | 2))
        {
            return None;
        }

        let mut ordered = Vec::with_capacity(entity_indices.len());
        let mut cursor = start.1;
        while ordered.len() < entity_indices.len() {
            let next = entity_indices.iter().find_map(|&index| {
                if ordered.contains(&index) {
                    return None;
                }
                let (a, b) = endpoints(index)?;
                if near(a, cursor) {
                    Some((index, b))
                } else if near(b, cursor) {
                    Some((index, a))
                } else {
                    None
                }
            })?;
            ordered.push(next.0);
            cursor = next.1;
        }
        Some(ordered)
    }

    fn entity_endpoints(&self, index: usize) -> Option<(DVec2, DVec2)> {
        match &self.entities.get(index)?.geo {
            SketchEntity::Line { a, b } => Some((*a, *b)),
            SketchEntity::Arc { start, end, .. } => Some((*start, *end)),
            SketchEntity::Spline { points } if points.len() >= 2 => {
                Some((points[0], *points.last()?))
            }
            SketchEntity::CvSpline { control, .. } if control.len() >= 2 => {
                Some((control[0], *control.last()?))
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => Some((
                ellipse_point(*center, *major, *minor_ratio, *start_angle),
                ellipse_point(*center, *major, *minor_ratio, *end_angle),
            )),
            SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => None,
            SketchEntity::Spline { .. }
            | SketchEntity::CvSpline { .. }
            | SketchEntity::Point { .. } => None,
        }
    }

    /// Converts a detected profile to its closed OpenCASCADE boundary wire.
    pub fn to_wire(&self, profile: &Profile) -> Option<Wire> {
        let edges = match profile {
            Profile::LineLoop(points) if points.len() >= 3 => points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .take(points.len())
                .map(|(a, b)| Edge::segment(self.plane.to_world(*a), self.plane.to_world(*b)).ok())
                .collect::<Option<Vec<_>>>()?,
            Profile::Circle { center, radius } if *radius > ENDPOINT_EPSILON => vec![
                Edge::circle(self.plane.to_world(*center), self.plane.normal(), *radius).ok()?,
            ],
            Profile::Ellipse {
                center,
                major,
                minor_ratio,
            } if major.length() > ENDPOINT_EPSILON && *minor_ratio > ENDPOINT_EPSILON => {
                let major_radius = major.length();
                let major_direction = self.plane.x_axis * major.x + self.plane.y_axis * major.y;
                vec![
                    Edge::ellipse(
                        self.plane.to_world(*center),
                        self.plane.normal(),
                        major_direction.normalize_or_zero(),
                        major_radius,
                        major_radius * minor_ratio.abs(),
                    )
                    .ok()?,
                ]
            }
            Profile::CurveLoop(curves) if curves.len() >= 2 => curves
                .iter()
                .map(|(index, reversed)| self.entity_edge(*index, *reversed))
                .collect::<Option<Vec<_>>>()?,
            _ => return None,
        };
        Wire::from_edges(edges).ok()
    }

    fn entity_edge(&self, index: usize, reversed: bool) -> Option<Edge> {
        match &self.entities.get(index)?.geo {
            SketchEntity::Line { a, b } => {
                let (a, b) = if reversed { (b, a) } else { (a, b) };
                Edge::segment(self.plane.to_world(*a), self.plane.to_world(*b)).ok()
            }
            SketchEntity::Arc { start, end, mid } => {
                let (start, end) = if reversed { (end, start) } else { (start, end) };
                let tangent = arc_start_tangent(*start, *mid, *end)?;
                Edge::tangent_arc(
                    self.plane.to_world(*start),
                    self.plane.x_axis * tangent.x + self.plane.y_axis * tangent.y,
                    self.plane.to_world(*end),
                )
                .ok()
            }
            SketchEntity::Spline { points } if points.len() >= 2 => {
                let points = if reversed {
                    points.iter().rev().copied().collect::<Vec<_>>()
                } else {
                    points.clone()
                };
                let points: Vec<_> = points
                    .into_iter()
                    .map(|point| self.plane.to_world(point))
                    .collect();
                Edge::spline_from_points(&points).ok()
            }
            SketchEntity::CvSpline { control, degree } if control.len() > *degree as usize => {
                let control: Vec<_> = if reversed {
                    control.iter().rev().copied().collect()
                } else {
                    control.clone()
                }
                .into_iter()
                .map(|point| self.plane.to_world(point))
                .collect();
                Edge::bspline_from_poles(&control, *degree).ok()
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => {
                let major_radius = major.length();
                let major_direction = self.plane.x_axis * major.x + self.plane.y_axis * major.y;
                let (start, end) = if reversed {
                    (*end_angle, *start_angle)
                } else {
                    (*start_angle, *end_angle)
                };
                Edge::ellipse_arc(
                    self.plane.to_world(*center),
                    self.plane.normal(),
                    major_direction.normalize_or_zero(),
                    major_radius,
                    major_radius * minor_ratio.abs(),
                    start,
                    end,
                )
                .ok()
            }
            SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => None,
            SketchEntity::Spline { .. }
            | SketchEntity::CvSpline { .. }
            | SketchEntity::Point { .. } => None,
        }
    }

    /// Converts selected line or arc entities into one connected open trajectory wire.
    pub fn open_chain_wire(&self, entity_indices: &[usize]) -> Option<Wire> {
        let ordered = self.order_open_chain(entity_indices)?;
        if ordered.len() != entity_indices.len() {
            return None;
        }
        let mut cursor = match &self.entities[ordered[0]].geo {
            SketchEntity::Line { a, b } => {
                let next_point = ordered
                    .get(1)
                    .and_then(|&next| match &self.entities[next].geo {
                        SketchEntity::Line { a, b } => Some((*a, *b)),
                        SketchEntity::Arc { start, end, .. } => Some((*start, *end)),
                        SketchEntity::Spline { points } if points.len() >= 2 => {
                            Some((points[0], *points.last()?))
                        }
                        SketchEntity::CvSpline { control, .. } if control.len() >= 2 => {
                            Some((control[0], *control.last()?))
                        }
                        SketchEntity::EllipseArc {
                            center,
                            major,
                            minor_ratio,
                            start_angle,
                            end_angle,
                        } => Some((
                            ellipse_point(*center, *major, *minor_ratio, *start_angle),
                            ellipse_point(*center, *major, *minor_ratio, *end_angle),
                        )),
                        SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => None,
                        SketchEntity::Spline { .. }
                        | SketchEntity::CvSpline { .. }
                        | SketchEntity::Point { .. } => None,
                    });
                if next_point.is_some_and(|(c, d)| near(*a, c) || near(*a, d)) {
                    *b
                } else {
                    *a
                }
            }
            SketchEntity::Arc { start, end, .. } => {
                let next_point = ordered.get(1).and_then(|&next| self.entity_endpoints(next));
                if next_point.is_some_and(|(a, b)| near(*start, a) || near(*start, b)) {
                    *end
                } else {
                    *start
                }
            }
            SketchEntity::Spline { points } if points.len() >= 2 => {
                let next_point = ordered.get(1).and_then(|&next| self.entity_endpoints(next));
                if next_point.is_some_and(|(a, b)| near(points[0], a) || near(points[0], b)) {
                    *points.last().expect("spline has endpoints")
                } else {
                    points[0]
                }
            }
            SketchEntity::CvSpline { control, .. } if control.len() >= 2 => {
                let next_point = ordered.get(1).and_then(|&next| self.entity_endpoints(next));
                if next_point.is_some_and(|(a, b)| near(control[0], a) || near(control[0], b)) {
                    *control.last().expect("CV spline has endpoints")
                } else {
                    control[0]
                }
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => {
                let next_point = ordered.get(1).and_then(|&next| self.entity_endpoints(next));
                let start = ellipse_point(*center, *major, *minor_ratio, *start_angle);
                let end = ellipse_point(*center, *major, *minor_ratio, *end_angle);
                if next_point.is_some_and(|(a, b)| near(start, a) || near(start, b)) {
                    end
                } else {
                    start
                }
            }
            SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => return None,
            SketchEntity::Spline { .. }
            | SketchEntity::CvSpline { .. }
            | SketchEntity::Point { .. } => return None,
        };
        let mut edges = Vec::with_capacity(ordered.len());
        for index in ordered {
            let (a, b) = self.entity_endpoints(index)?;
            let endpoint = if near(a, cursor) {
                b
            } else if near(b, cursor) {
                a
            } else {
                return None;
            };
            edges.push(self.entity_edge(index, near(b, cursor))?);
            cursor = endpoint;
        }
        Wire::from_edges(edges).ok()
    }

    /// Converts a detected profile to a planar OpenCASCADE face.
    pub fn to_face(&self, profile: &Profile) -> Option<Face> {
        let wire = self.to_wire(profile)?;
        Face::from_wire(&wire).ok()
    }

    /// Rounds a shared endpoint of two lines with a tangent circular arc.
    pub fn fillet_lines(&mut self, first: usize, second: usize, radius: f64) -> bool {
        if first == second || !radius.is_finite() || radius <= ENDPOINT_EPSILON {
            return false;
        }
        let Some(SketchItem {
            geo: SketchEntity::Line { a: a0, b: a1 },
            ..
        }) = self.entities.get(first).cloned()
        else {
            return false;
        };
        let Some(SketchItem {
            geo: SketchEntity::Line { a: b0, b: b1 },
            ..
        }) = self.entities.get(second).cloned()
        else {
            return false;
        };
        let Some((corner, other_a, point_a, other_b, point_b)) = shared_line_corner(a0, a1, b0, b1)
        else {
            return false;
        };
        let ray_a = (other_a - corner).normalize_or_zero();
        let ray_b = (other_b - corner).normalize_or_zero();
        let angle = ray_a.dot(ray_b).clamp(-1.0, 1.0).acos();
        let tangent = radius / (angle * 0.5).tan();
        if !tangent.is_finite()
            || tangent >= corner.distance(other_a) - ENDPOINT_EPSILON
            || tangent >= corner.distance(other_b) - ENDPOINT_EPSILON
        {
            return false;
        }
        let start = corner + ray_a * tangent;
        let end = corner + ray_b * tangent;
        let bisector = (ray_a + ray_b).normalize_or_zero();
        let center_distance = radius / (angle * 0.5).sin();
        let center = corner + bisector * center_distance;
        let toward_corner = (corner - center).normalize_or_zero();
        let mid = center + toward_corner * radius;
        set_line_point(&mut self.entities[first].geo, point_a, start);
        set_line_point(&mut self.entities[second].geo, point_b, end);
        self.constraints.retain(|constraint| {
            !matches!(
                constraint,
                Constraint::Coincident { a, b }
                    if (a.entity == first
                        && a.point == point_a
                        && b.entity == second
                        && b.point == point_b)
                        || (b.entity == first
                            && b.point == point_a
                            && a.entity == second
                            && a.point == point_b)
            )
        });
        let arc = self.entities.len();
        self.entities
            .push(SketchEntity::Arc { start, end, mid }.into());
        self.constraints.extend([
            Constraint::Coincident {
                a: PointRef {
                    entity: arc,
                    point: 0,
                },
                b: PointRef {
                    entity: first,
                    point: point_a,
                },
            },
            Constraint::Coincident {
                a: PointRef {
                    entity: arc,
                    point: 1,
                },
                b: PointRef {
                    entity: second,
                    point: point_b,
                },
            },
            Constraint::Tangent {
                line: EntityRef(first),
                circle: EntityRef(arc),
            },
            Constraint::Tangent {
                line: EntityRef(second),
                circle: EntityRef(arc),
            },
        ]);
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }

    /// Removes the portion of an entity surrounding `pick`, splitting survivors.
    pub fn trim(&mut self, entity: usize, pick: DVec2) -> bool {
        let Some(source) = self.entities.get(entity).map(|item| item.geo.clone()) else {
            return false;
        };
        let intersections = self.trim_intersections(entity, &source);
        let replacements = match source {
            SketchEntity::Line { a, b } => trim_line(a, b, pick, &intersections),
            SketchEntity::Circle { center, radius } => {
                trim_circle(center, radius, pick, &intersections)
            }
            SketchEntity::Arc { start, end, mid } => {
                trim_arc(start, mid, end, pick, &intersections)
            }
            SketchEntity::Spline { .. } => None,
            SketchEntity::CvSpline { .. } => None,
            SketchEntity::Ellipse { .. } => None,
            // TODO(W2): split ellipse arcs once parameter-space intersection trimming is available.
            // Callers currently reject the operation with a soft readout hint.
            SketchEntity::EllipseArc { .. } | SketchEntity::Point { .. } => None,
        };
        let Some(replacements) = replacements else {
            return false;
        };

        // Split constraints are intentionally conservative: constraints on the
        // removed source are dropped, while later entity indices are remapped.
        // Preserving arbitrary constraint intent across one-to-many splits is
        // outside this trim operation's scope.
        let old_len = self.entities.len();
        self.entities.remove(entity);
        self.entities
            .extend(replacements.into_iter().map(SketchItem::regular));
        let map: Vec<_> = (0..old_len)
            .map(|old| {
                if old == entity {
                    None
                } else if old > entity {
                    Some(old - 1)
                } else {
                    Some(old)
                }
            })
            .collect();
        self.constraints.retain_mut(|constraint| {
            !constraint.references_entity(entity) && constraint.remap_entities(&map)
        });
        self.pinned.clear();
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }

    /// Extends the endpoint nearest `pick` to the closest forward intersection.
    pub fn extend(&mut self, entity: usize, pick: DVec2) -> bool {
        let Some(source) = self.entities.get(entity).map(|item| item.geo.clone()) else {
            return false;
        };
        let replacement = match source {
            SketchEntity::Line { a, b } => {
                let extend_start = pick.distance(a) <= pick.distance(b);
                let (endpoint, inward) = if extend_start { (a, b - a) } else { (b, a - b) };
                let outward = -inward.normalize_or_zero();
                self.entities
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != entity)
                    .flat_map(|(_, other)| {
                        infinite_line_entity_intersections(endpoint, outward, &other.geo)
                    })
                    .filter_map(|point| {
                        let distance = (point - endpoint).dot(outward);
                        (distance > ENDPOINT_EPSILON).then_some((distance, point))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, point)| {
                        if extend_start {
                            SketchEntity::Line { a: point, b }
                        } else {
                            SketchEntity::Line { a, b: point }
                        }
                    })
            }
            SketchEntity::Arc { start, mid, end } => {
                extend_arc(self, entity, start, mid, end, pick)
            }
            _ => None,
        };
        let Some(replacement) = replacement else {
            return false;
        };
        self.entities[entity].geo = replacement;
        self.pinned.clear();
        self.refresh_spline_samples();
        true
    }

    /// Splits a curve at its nearest parameter without removing any geometry.
    pub fn break_at(&mut self, entity: usize, pick: DVec2) -> bool {
        let Some(item) = self.entities.get(entity).cloned() else {
            return false;
        };
        let replacements = match item.geo {
            SketchEntity::Line { a, b } => {
                let direction = b - a;
                let t = ((pick - a).dot(direction) / direction.length_squared()).clamp(0.0, 1.0);
                let split = a + direction * t;
                (t > ENDPOINT_EPSILON && t < 1.0 - ENDPOINT_EPSILON).then(|| {
                    vec![
                        SketchEntity::Line { a, b: split },
                        SketchEntity::Line { a: split, b },
                    ]
                })
            }
            SketchEntity::Arc { start, mid, end } => {
                let Some((center, radius)) = arc_center_radius(start, mid, end) else {
                    return false;
                };
                let Some(t) = arc_parameter(center, start, mid, end, pick) else {
                    return false;
                };
                let t = t.clamp(0.0, 1.0);
                if t <= ENDPOINT_EPSILON || t >= 1.0 - ENDPOINT_EPSILON {
                    return false;
                }
                let start_angle = (start.y - center.y).atan2(start.x - center.x);
                let end_angle = (end.y - center.y).atan2(end.x - center.x);
                let mut sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
                if ((mid.y - center.y).atan2(mid.x - center.x) - start_angle)
                    .rem_euclid(std::f64::consts::TAU)
                    > sweep
                {
                    sweep -= std::f64::consts::TAU;
                }
                Some(vec![
                    arc_from_angles(center, radius, start_angle, sweep * t),
                    arc_from_angles(center, radius, start_angle + sweep * t, sweep * (1.0 - t)),
                ])
            }
            SketchEntity::Circle { center, radius } => {
                let direction = (pick - center).normalize_or_zero();
                if direction.length_squared() < 0.5 || radius <= ENDPOINT_EPSILON {
                    return false;
                }
                let angle = direction.y.atan2(direction.x);
                Some(vec![
                    arc_from_angles(center, radius, angle, std::f64::consts::PI),
                    arc_from_angles(
                        center,
                        radius,
                        angle + std::f64::consts::PI,
                        std::f64::consts::PI,
                    ),
                ])
            }
            _ => None,
        };
        let Some(replacements) = replacements else {
            return false;
        };
        self.replace_split_entity(entity, item.construction, replacements)
    }

    fn replace_split_entity(
        &mut self,
        entity: usize,
        construction: bool,
        replacements: Vec<SketchEntity>,
    ) -> bool {
        let old_len = self.entities.len();
        self.entities.remove(entity);
        self.entities.extend(
            replacements
                .into_iter()
                .map(|geo| SketchItem { geo, construction }),
        );
        let map: Vec<_> = (0..old_len)
            .map(|old| {
                if old == entity {
                    None
                } else if old > entity {
                    Some(old - 1)
                } else {
                    Some(old)
                }
            })
            .collect();
        self.constraints.retain_mut(|constraint| {
            !constraint.references_entity(entity) && constraint.remap_entities(&map)
        });
        self.pinned.clear();
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }

    /// Returns the exact sub-segment that trim would remove at `pick`.
    pub fn trim_subsegment(&self, entity: usize, pick: DVec2) -> Option<SketchEntity> {
        let source = &self.entities.get(entity)?.geo;
        let intersections = self.trim_intersections(entity, source);
        match source {
            SketchEntity::Line { a, b } => removed_line(*a, *b, pick, &intersections),
            SketchEntity::Circle { center, radius } => {
                removed_circle(*center, *radius, pick, &intersections)
            }
            SketchEntity::Arc { start, end, mid } => {
                removed_arc(*start, *mid, *end, pick, &intersections)
            }
            SketchEntity::Spline { .. } => None,
            SketchEntity::CvSpline { .. } => None,
            SketchEntity::Ellipse { .. } => None,
            SketchEntity::EllipseArc { .. } | SketchEntity::Point { .. } => None,
        }
    }

    fn trim_intersections(&self, entity: usize, source: &SketchEntity) -> Vec<DVec2> {
        let mut intersections = self
            .entities
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != entity)
            .flat_map(|(_, other)| entity_intersections(source, &other.geo))
            .collect::<Vec<_>>();
        deduplicate_points(&mut intersections);
        intersections
    }

    /// Appends an offset copy of one detected closed profile.
    pub fn offset_profile(&mut self, profile_index: usize, distance: f64) -> bool {
        if !distance.is_finite() || distance.abs() <= ENDPOINT_EPSILON {
            return false;
        }
        let Some(profile) = self.profiles().get(profile_index).cloned() else {
            return false;
        };
        let additions = match profile {
            Profile::LineLoop(points) => offset_polygon(&points, distance),
            Profile::Circle { center, radius } => {
                let radius = radius + distance;
                (radius > ENDPOINT_EPSILON).then(|| vec![SketchEntity::Circle { center, radius }])
            }
            Profile::Ellipse {
                center,
                major,
                minor_ratio,
            } => {
                let major_radius = major.length() + distance;
                let minor_radius = major.length() * minor_ratio + distance;
                (major_radius > ENDPOINT_EPSILON && minor_radius > ENDPOINT_EPSILON).then(|| {
                    vec![SketchEntity::Ellipse {
                        center,
                        major: major.normalize_or_zero() * major_radius,
                        minor_ratio: minor_radius / major_radius,
                    }]
                })
            }
            Profile::CurveLoop(curves) => {
                let entities: Vec<_> = self.entities.iter().map(|item| item.geo.clone()).collect();
                offset_curve_loop(&entities, &curves, distance)
            }
        };
        let Some(additions) = additions else {
            return false;
        };
        self.entities
            .extend(additions.into_iter().map(SketchItem::regular));
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }

    /// Appends reflected copies of selected entities and endpoint symmetry constraints.
    pub fn mirror_entities(&mut self, entities: &[usize], axis: usize) -> bool {
        let Some(SketchEntity::Line { a, b }) = self.entities.get(axis).map(|item| &item.geo)
        else {
            return false;
        };
        let (axis_a, axis_b) = (*a, *b);
        if axis_a.distance(axis_b) <= ENDPOINT_EPSILON {
            return false;
        }
        let sources: Vec<_> = entities
            .iter()
            .copied()
            .filter(|&index| index != axis)
            .filter_map(|index| self.entities.get(index).cloned().map(|item| (index, item)))
            .collect();
        if sources.is_empty() {
            return false;
        }
        for (original, mut copy) in sources {
            copy.geo = mirrored_entity(&copy.geo, axis_a, axis_b);
            let copied = self.entities.len();
            let endpoints = endpoint_count(&copy.geo);
            self.entities.push(copy);
            for point in 0..endpoints {
                self.constraints.push(Constraint::Symmetric {
                    a: PointRef {
                        entity: original,
                        point,
                    },
                    b: PointRef {
                        entity: copied,
                        point,
                    },
                    axis: EntityRef(axis),
                });
            }
        }
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }

    /// Appends translated copies without propagating source constraints.
    pub fn linear_pattern(
        &mut self,
        entities: &[usize],
        direction: DVec2,
        count: usize,
        spacing: f64,
    ) -> bool {
        let direction = direction.normalize_or_zero();
        if entities.is_empty()
            || direction.length_squared() < 0.99
            || count < 2
            || !spacing.is_finite()
        {
            return false;
        }
        let sources: Vec<_> = entities
            .iter()
            .filter_map(|&index| self.entities.get(index).cloned())
            .collect();
        if sources.is_empty() {
            return false;
        }
        for instance in 1..count.clamp(2, 12) {
            let offset = direction * spacing * instance as f64;
            self.entities
                .extend(sources.iter().cloned().map(|mut item| {
                    item.geo =
                        transformed_entity(&item.geo, |point| point + offset, |vector| vector);
                    item
                }));
        }
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }

    /// Appends evenly rotated copies about a plane-local center.
    pub fn circular_pattern(&mut self, entities: &[usize], center: DVec2, count: usize) -> bool {
        if entities.is_empty() || count < 2 {
            return false;
        }
        let sources: Vec<_> = entities
            .iter()
            .filter_map(|&index| self.entities.get(index).cloned())
            .collect();
        if sources.is_empty() {
            return false;
        }
        for instance in 1..count.clamp(2, 12) {
            let angle = std::f64::consts::TAU * instance as f64 / count.clamp(2, 12) as f64;
            let rotate = |vector: DVec2| {
                let (sin, cos) = angle.sin_cos();
                DVec2::new(
                    vector.x * cos - vector.y * sin,
                    vector.x * sin + vector.y * cos,
                )
            };
            self.entities
                .extend(sources.iter().cloned().map(|mut item| {
                    item.geo = transformed_entity(
                        &item.geo,
                        |point| center + rotate(point - center),
                        rotate,
                    );
                    item
                }));
        }
        self.defined.resize(self.entities.len(), false);
        self.refresh_spline_samples();
        true
    }
}

fn endpoint_count(entity: &SketchEntity) -> u8 {
    match entity {
        SketchEntity::Line { .. } | SketchEntity::Arc { .. } | SketchEntity::EllipseArc { .. } => 2,
        SketchEntity::Spline { points } if points.len() >= 2 => 2,
        SketchEntity::CvSpline { control, .. } if control.len() >= 2 => 2,
        _ => 0,
    }
}

fn transformed_entity(
    entity: &SketchEntity,
    point: impl Fn(DVec2) -> DVec2,
    vector: impl Fn(DVec2) -> DVec2,
) -> SketchEntity {
    match entity {
        SketchEntity::Line { a, b } => SketchEntity::Line {
            a: point(*a),
            b: point(*b),
        },
        SketchEntity::Circle { center, radius } => SketchEntity::Circle {
            center: point(*center),
            radius: *radius,
        },
        SketchEntity::Ellipse {
            center,
            major,
            minor_ratio,
        } => SketchEntity::Ellipse {
            center: point(*center),
            major: vector(*major),
            minor_ratio: *minor_ratio,
        },
        SketchEntity::Arc { start, end, mid } => SketchEntity::Arc {
            start: point(*start),
            end: point(*end),
            mid: point(*mid),
        },
        SketchEntity::Spline { points } => SketchEntity::Spline {
            points: points.iter().copied().map(point).collect(),
        },
        SketchEntity::CvSpline { control, degree } => SketchEntity::CvSpline {
            control: control.iter().copied().map(point).collect(),
            degree: *degree,
        },
        SketchEntity::EllipseArc {
            center,
            major,
            minor_ratio,
            start_angle,
            end_angle,
        } => SketchEntity::EllipseArc {
            center: point(*center),
            major: vector(*major),
            minor_ratio: *minor_ratio,
            start_angle: *start_angle,
            end_angle: *end_angle,
        },
        SketchEntity::Point { at } => SketchEntity::Point { at: point(*at) },
    }
}

fn mirrored_entity(entity: &SketchEntity, axis_a: DVec2, axis_b: DVec2) -> SketchEntity {
    let direction = (axis_b - axis_a).normalize();
    let reflect_vector = |vector: DVec2| direction * (2.0 * vector.dot(direction)) - vector;
    let reflect_point = |point: DVec2| axis_a + reflect_vector(point - axis_a);
    let mut mirrored = transformed_entity(entity, reflect_point, reflect_vector);
    if let SketchEntity::EllipseArc {
        start_angle,
        end_angle,
        ..
    } = &mut mirrored
    {
        *start_angle = -*start_angle;
        *end_angle = -*end_angle;
    }
    mirrored
}

fn shared_line_corner(
    a0: DVec2,
    a1: DVec2,
    b0: DVec2,
    b1: DVec2,
) -> Option<(DVec2, DVec2, u8, DVec2, u8)> {
    if near(a0, b0) {
        Some((a0, a1, 0, b1, 0))
    } else if near(a0, b1) {
        Some((a0, a1, 0, b0, 1))
    } else if near(a1, b0) {
        Some((a1, a0, 1, b1, 0))
    } else if near(a1, b1) {
        Some((a1, a0, 1, b0, 1))
    } else {
        None
    }
}

fn set_line_point(entity: &mut SketchEntity, point: u8, value: DVec2) {
    let SketchEntity::Line { a, b } = entity else {
        return;
    };
    if point == 0 {
        *a = value;
    } else {
        *b = value;
    }
}

/// Returns all finite intersections shared by two sketch entities.
pub fn entity_intersections(a: &SketchEntity, b: &SketchEntity) -> Vec<DVec2> {
    if matches!(
        a,
        SketchEntity::Spline { .. } | SketchEntity::CvSpline { .. }
    ) || matches!(
        b,
        SketchEntity::Spline { .. } | SketchEntity::CvSpline { .. }
    ) {
        return Vec::new();
    }
    match (a, b) {
        (SketchEntity::Line { a: a0, b: a1 }, SketchEntity::Line { a: b0, b: b1 }) => {
            line_line(*a0, *a1, *b0, *b1).into_iter().collect()
        }
        (SketchEntity::Line { a, b }, circular) | (circular, SketchEntity::Line { a, b }) => {
            circle_data(circular)
                .map(|(center, radius)| line_circle(*a, *b, center, radius))
                .unwrap_or_default()
                .into_iter()
                .filter(|point| arc_contains(circular, *point))
                .collect()
        }
        (a, b) => {
            let (Some((ca, ra)), Some((cb, rb))) = (circle_data(a), circle_data(b)) else {
                return Vec::new();
            };
            circle_circle(ca, ra, cb, rb)
                .into_iter()
                .filter(|point| arc_contains(a, *point) && arc_contains(b, *point))
                .collect()
        }
    }
}

fn infinite_line_entity_intersections(
    origin: DVec2,
    direction: DVec2,
    other: &SketchEntity,
) -> Vec<DVec2> {
    match other {
        SketchEntity::Line { a, b } => {
            infinite_line_intersection(origin, origin + direction, *a, *b)
                .filter(|point| {
                    let segment = *b - *a;
                    let t = (*point - *a).dot(segment) / segment.length_squared();
                    (-ENDPOINT_EPSILON..=1.0 + ENDPOINT_EPSILON).contains(&t)
                })
                .into_iter()
                .collect()
        }
        circular => circle_data(circular)
            .map(|(center, radius)| infinite_line_circle(origin, direction, center, radius))
            .unwrap_or_default()
            .into_iter()
            .filter(|point| arc_contains(circular, *point))
            .collect(),
    }
}

fn infinite_line_circle(origin: DVec2, direction: DVec2, center: DVec2, radius: f64) -> Vec<DVec2> {
    let relative = origin - center;
    let aa = direction.length_squared();
    let bb = 2.0 * relative.dot(direction);
    let cc = relative.length_squared() - radius * radius;
    let discriminant = bb * bb - 4.0 * aa * cc;
    if aa <= 1.0e-12 || discriminant < -1.0e-12 {
        return Vec::new();
    }
    let root = discriminant.max(0.0).sqrt();
    [-root, root]
        .into_iter()
        .map(|signed| origin + direction * ((-bb + signed) / (2.0 * aa)))
        .collect()
}

fn extend_arc(
    sketch: &Sketch,
    entity: usize,
    start: DVec2,
    mid: DVec2,
    end: DVec2,
    pick: DVec2,
) -> Option<SketchEntity> {
    let (center, radius) = arc_center_radius(start, mid, end)?;
    let angle = |point: DVec2| (point.y - center.y).atan2(point.x - center.x);
    let start_angle = angle(start);
    let end_angle = angle(end);
    let mut sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
    if (angle(mid) - start_angle).rem_euclid(std::f64::consts::TAU) > sweep {
        sweep -= std::f64::consts::TAU;
    }
    let extend_start = pick.distance(start) <= pick.distance(end);
    let sign = sweep.signum();
    let candidates = sketch
        .entities
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != entity)
        .flat_map(|(_, other)| {
            entity_intersections(&SketchEntity::Circle { center, radius }, &other.geo)
        });
    let best = candidates
        .filter_map(|point| {
            let delta = if extend_start {
                ((start_angle - angle(point)) * sign).rem_euclid(std::f64::consts::TAU)
            } else {
                ((angle(point) - end_angle) * sign).rem_euclid(std::f64::consts::TAU)
            };
            (delta > 1.0e-8 && delta < std::f64::consts::TAU - sweep.abs() - 1.0e-8)
                .then_some((delta, point))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?
        .1;
    let (new_start, new_end) = if extend_start {
        (angle(best), end_angle)
    } else {
        (start_angle, angle(best))
    };
    let mut new_sweep = (new_end - new_start).rem_euclid(std::f64::consts::TAU);
    if sign < 0.0 {
        new_sweep -= std::f64::consts::TAU;
    }
    Some(arc_from_angles(center, radius, new_start, new_sweep))
}

/// Circle tangent to two infinite lines in the quadrant nearest `side_hint`.
pub fn two_tangent_circle(
    a0: DVec2,
    a1: DVec2,
    b0: DVec2,
    b1: DVec2,
    radius: f64,
    side_hint: DVec2,
) -> Option<(DVec2, f64)> {
    if radius <= ENDPOINT_EPSILON {
        return None;
    }
    let intersection = infinite_line_intersection(a0, a1, b0, b1)?;
    let da = (a1 - a0).normalize_or_zero();
    let db = (b1 - b0).normalize_or_zero();
    let normals = [DVec2::new(-da.y, da.x), DVec2::new(-db.y, db.x)];
    [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)]
        .into_iter()
        .filter_map(|(sa, sb)| {
            let pa = a0 + normals[0] * radius * sa;
            let pb = b0 + normals[1] * radius * sb;
            infinite_line_intersection(pa, pa + da, pb, pb + db)
        })
        .min_by(|a, b| a.distance(side_hint).total_cmp(&b.distance(side_hint)))
        .map(|center| (center, radius))
        .filter(|(center, _)| center.distance(intersection).is_finite())
}

/// Incircle tangent to three non-parallel infinite lines.
pub fn three_tangent_circle(lines: [(DVec2, DVec2); 3]) -> Option<(DVec2, f64)> {
    let vertices = [
        infinite_line_intersection(lines[0].0, lines[0].1, lines[1].0, lines[1].1)?,
        infinite_line_intersection(lines[1].0, lines[1].1, lines[2].0, lines[2].1)?,
        infinite_line_intersection(lines[2].0, lines[2].1, lines[0].0, lines[0].1)?,
    ];
    let lengths = [
        vertices[1].distance(vertices[2]),
        vertices[2].distance(vertices[0]),
        vertices[0].distance(vertices[1]),
    ];
    let perimeter: f64 = lengths.iter().sum();
    if perimeter <= ENDPOINT_EPSILON {
        return None;
    }
    let center = (vertices[0] * lengths[0] + vertices[1] * lengths[1] + vertices[2] * lengths[2])
        / perimeter;
    let direction = lines[0].1 - lines[0].0;
    let radius = direction.perp_dot(center - lines[0].0).abs() / direction.length();
    (radius > ENDPOINT_EPSILON && radius.is_finite()).then_some((center, radius))
}

fn line_line(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> Option<DVec2> {
    let ab = b - a;
    let cd = d - c;
    let denominator = ab.perp_dot(cd);
    if denominator.abs() <= 1.0e-12 {
        return None;
    }
    let t = (c - a).perp_dot(cd) / denominator;
    let u = (c - a).perp_dot(ab) / denominator;
    ((-ENDPOINT_EPSILON..=1.0 + ENDPOINT_EPSILON).contains(&t)
        && (-ENDPOINT_EPSILON..=1.0 + ENDPOINT_EPSILON).contains(&u))
    .then_some(a + ab * t)
}

fn line_circle(a: DVec2, b: DVec2, center: DVec2, radius: f64) -> Vec<DVec2> {
    let direction = b - a;
    let relative = a - center;
    let aa = direction.length_squared();
    let bb = 2.0 * relative.dot(direction);
    let cc = relative.length_squared() - radius * radius;
    let discriminant = bb * bb - 4.0 * aa * cc;
    if aa <= 1.0e-12 || discriminant < -1.0e-12 {
        return Vec::new();
    }
    let root = discriminant.max(0.0).sqrt();
    [-root, root]
        .into_iter()
        .map(|signed| (-bb + signed) / (2.0 * aa))
        .filter(|t| (-ENDPOINT_EPSILON..=1.0 + ENDPOINT_EPSILON).contains(t))
        .map(|t| a + direction * t)
        .collect()
}

fn circle_circle(a: DVec2, ra: f64, b: DVec2, rb: f64) -> Vec<DVec2> {
    let delta = b - a;
    let distance = delta.length();
    if distance <= 1.0e-12
        || distance > ra + rb + ENDPOINT_EPSILON
        || distance < (ra - rb).abs() - ENDPOINT_EPSILON
    {
        return Vec::new();
    }
    let along = (ra * ra - rb * rb + distance * distance) / (2.0 * distance);
    let height = (ra * ra - along * along).max(0.0).sqrt();
    let base = a + delta / distance * along;
    let perpendicular = DVec2::new(-delta.y, delta.x) / distance * height;
    vec![base + perpendicular, base - perpendicular]
}

fn circle_data(entity: &SketchEntity) -> Option<(DVec2, f64)> {
    match entity {
        SketchEntity::Circle { center, radius } => Some((*center, *radius)),
        SketchEntity::Arc { start, end, mid } => arc_center_radius(*start, *mid, *end),
        SketchEntity::Line { .. } => None,
        SketchEntity::Spline { .. }
        | SketchEntity::CvSpline { .. }
        | SketchEntity::Ellipse { .. }
        | SketchEntity::EllipseArc { .. }
        | SketchEntity::Point { .. } => None,
    }
}

/// Samples the same OCCT interpolation used for wire construction.
pub fn sample_spline(points: &[DVec2], plane: SketchPlane) -> Vec<DVec2> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let world: Vec<_> = points.iter().map(|point| plane.to_world(*point)).collect();
    Edge::spline_from_points(&world)
        .and_then(|edge| edge.as_shape().edge_polyline(0, 0.1))
        .map(|points| {
            points
                .into_iter()
                .map(|point| plane.to_local(point))
                .collect()
        })
        .unwrap_or_else(|_| points.to_vec())
}

/// Samples a clamped uniform control-point B-spline through OCCT.
pub fn sample_cv_spline(control: &[DVec2], degree: u8, plane: SketchPlane) -> Vec<DVec2> {
    if control.len() <= degree as usize || degree == 0 {
        return control.to_vec();
    }
    let world: Vec<_> = control.iter().map(|point| plane.to_world(*point)).collect();
    Edge::bspline_from_poles(&world, degree)
        .and_then(|edge| edge.as_shape().edge_polyline(0, 0.1))
        .map(|points| {
            points
                .into_iter()
                .map(|point| plane.to_local(point))
                .collect()
        })
        .unwrap_or_else(|_| control.to_vec())
}

/// Samples a full ellipse in counter-clockwise order.
pub fn sample_ellipse(
    center: DVec2,
    major: DVec2,
    minor_ratio: f64,
    segments: usize,
) -> Vec<DVec2> {
    let minor = DVec2::new(-major.y, major.x) * minor_ratio;
    (0..=segments)
        .map(|index| {
            let angle = index as f64 / segments.max(1) as f64 * std::f64::consts::TAU;
            center + major * angle.cos() + minor * angle.sin()
        })
        .collect()
}

/// Evaluates an ellipse-local angular parameter.
pub fn ellipse_point(center: DVec2, major: DVec2, minor_ratio: f64, angle: f64) -> DVec2 {
    let minor = DVec2::new(-major.y, major.x) * minor_ratio;
    center + major * angle.cos() + minor * angle.sin()
}

/// Samples a partial ellipse from start to end angle.
pub fn sample_ellipse_arc(
    center: DVec2,
    major: DVec2,
    minor_ratio: f64,
    start_angle: f64,
    end_angle: f64,
    segments: usize,
) -> Vec<DVec2> {
    let sweep = end_angle - start_angle;
    (0..=segments.max(1))
        .map(|index| {
            ellipse_point(
                center,
                major,
                minor_ratio,
                start_angle + sweep * index as f64 / segments.max(1) as f64,
            )
        })
        .collect()
}

/// Computes the circumcircle through three non-collinear points.
pub fn three_point_circle(a: DVec2, b: DVec2, c: DVec2) -> Option<(DVec2, f64)> {
    arc_center_radius(a, b, c)
}

fn closed_coincident_constraints(count: usize) -> Vec<Constraint> {
    (0..count)
        .map(|index| Constraint::Coincident {
            a: PointRef {
                entity: index,
                point: 1,
            },
            b: PointRef {
                entity: (index + 1) % count,
                point: 0,
            },
        })
        .collect()
}

/// Creates a regular polygon whose first vertex is `vertex`.
pub fn regular_polygon(
    center: DVec2,
    vertex: DVec2,
    sides: usize,
) -> Option<(Vec<SketchEntity>, Vec<Constraint>)> {
    let sides = sides.clamp(3, 24);
    let radial = vertex - center;
    (radial.length() > ENDPOINT_EPSILON).then_some(())?;
    let vertices: Vec<_> = (0..sides)
        .map(|index| {
            let angle = std::f64::consts::TAU * index as f64 / sides as f64;
            let (sin, cos) = angle.sin_cos();
            center
                + DVec2::new(
                    radial.x * cos - radial.y * sin,
                    radial.x * sin + radial.y * cos,
                )
        })
        .collect();
    let entities = (0..sides)
        .map(|index| SketchEntity::Line {
            a: vertices[index],
            b: vertices[(index + 1) % sides],
        })
        .collect();
    Some((entities, closed_coincident_constraints(sides)))
}

/// Creates a four-line rectangle from a center and one corner.
pub fn centered_rectangle(
    center: DVec2,
    corner: DVec2,
) -> Option<(Vec<SketchEntity>, Vec<Constraint>)> {
    let half = (corner - center).abs();
    (half.min_element() > ENDPOINT_EPSILON).then_some(())?;
    let points = [
        center - half,
        center + DVec2::new(half.x, -half.y),
        center + half,
        center + DVec2::new(-half.x, half.y),
    ];
    let entities = (0..4)
        .map(|index| SketchEntity::Line {
            a: points[index],
            b: points[(index + 1) % 4],
        })
        .collect();
    let mut constraints = closed_coincident_constraints(4);
    constraints.extend([
        Constraint::Horizontal(EntityRef(0)),
        Constraint::Vertical(EntityRef(1)),
        Constraint::Horizontal(EntityRef(2)),
        Constraint::Vertical(EntityRef(3)),
    ]);
    Some((entities, constraints))
}

/// Creates a capsule slot from two cap centers and a positive radius.
pub fn slot(a: DVec2, b: DVec2, radius: f64) -> Option<(Vec<SketchEntity>, Vec<Constraint>)> {
    let axis = (b - a).normalize_or_zero();
    (axis.length_squared() > 0.99 && radius > ENDPOINT_EPSILON).then_some(())?;
    let normal = DVec2::new(-axis.y, axis.x);
    let entities = vec![
        SketchEntity::Line {
            a: a + normal * radius,
            b: b + normal * radius,
        },
        SketchEntity::Arc {
            start: b + normal * radius,
            mid: b + axis * radius,
            end: b - normal * radius,
        },
        SketchEntity::Line {
            a: b - normal * radius,
            b: a - normal * radius,
        },
        SketchEntity::Arc {
            start: a - normal * radius,
            mid: a - axis * radius,
            end: a + normal * radius,
        },
    ];
    let mut constraints = closed_coincident_constraints(4);
    constraints.extend([
        Constraint::Tangent {
            line: EntityRef(0),
            circle: EntityRef(1),
        },
        Constraint::Tangent {
            line: EntityRef(2),
            circle: EntityRef(1),
        },
        Constraint::Tangent {
            line: EntityRef(2),
            circle: EntityRef(3),
        },
        Constraint::Tangent {
            line: EntityRef(0),
            circle: EntityRef(3),
        },
    ]);
    Some((entities, constraints))
}

/// Creates an axis-aligned rounded rectangle; radius is clamped below half the short side.
pub fn rounded_rectangle(
    a: DVec2,
    b: DVec2,
    radius: f64,
) -> Option<(Vec<SketchEntity>, Vec<Constraint>)> {
    let min = a.min(b);
    let max = a.max(b);
    let size = max - min;
    (size.min_element() > ENDPOINT_EPSILON && radius > ENDPOINT_EPSILON).then_some(())?;
    let r = radius.min(size.min_element() * 0.5 - ENDPOINT_EPSILON);
    (r > ENDPOINT_EPSILON).then_some(())?;
    let q = std::f64::consts::FRAC_1_SQRT_2 * r;
    let entities = vec![
        SketchEntity::Line {
            a: DVec2::new(min.x + r, min.y),
            b: DVec2::new(max.x - r, min.y),
        },
        SketchEntity::Arc {
            start: DVec2::new(max.x - r, min.y),
            mid: DVec2::new(max.x - r + q, min.y + r - q),
            end: DVec2::new(max.x, min.y + r),
        },
        SketchEntity::Line {
            a: DVec2::new(max.x, min.y + r),
            b: DVec2::new(max.x, max.y - r),
        },
        SketchEntity::Arc {
            start: DVec2::new(max.x, max.y - r),
            mid: DVec2::new(max.x - r + q, max.y - r + q),
            end: DVec2::new(max.x - r, max.y),
        },
        SketchEntity::Line {
            a: DVec2::new(max.x - r, max.y),
            b: DVec2::new(min.x + r, max.y),
        },
        SketchEntity::Arc {
            start: DVec2::new(min.x + r, max.y),
            mid: DVec2::new(min.x + r - q, max.y - r + q),
            end: DVec2::new(min.x, max.y - r),
        },
        SketchEntity::Line {
            a: DVec2::new(min.x, max.y - r),
            b: DVec2::new(min.x, min.y + r),
        },
        SketchEntity::Arc {
            start: DVec2::new(min.x, min.y + r),
            mid: DVec2::new(min.x + r - q, min.y + r - q),
            end: DVec2::new(min.x + r, min.y),
        },
    ];
    let mut constraints = closed_coincident_constraints(8);
    for (line, arc) in [
        (0, 1),
        (2, 1),
        (2, 3),
        (4, 3),
        (4, 5),
        (6, 5),
        (6, 7),
        (0, 7),
    ] {
        constraints.push(Constraint::Tangent {
            line: EntityRef(line),
            circle: EntityRef(arc),
        });
    }
    Some((entities, constraints))
}

/// Computes the middle point of the unique start-tangent circular arc.
///
/// Tangent arcs are stored as the existing three-point [`SketchEntity::Arc`];
/// this preserves serialization and constraint compatibility while native OCCT
/// constructs the same circular geometry for profile wires.
pub fn tangent_arc_mid(start: DVec2, tangent: DVec2, end: DVec2) -> Option<DVec2> {
    let tangent = tangent.normalize_or_zero();
    let chord = end - start;
    if tangent.length_squared() < 0.99 || chord.length_squared() <= 1.0e-12 {
        return None;
    }
    let left = DVec2::new(-tangent.y, tangent.x);
    let denominator = 2.0 * left.dot(chord);
    if denominator.abs() <= 1.0e-12 {
        return None;
    }
    let signed_radius = chord.length_squared() / denominator;
    let center = start + left * signed_radius;
    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let end_angle = (end.y - center.y).atan2(end.x - center.x);
    let ccw_tangent = DVec2::new(-(start.y - center.y), start.x - center.x).normalize_or_zero();
    let sweep = if ccw_tangent.dot(tangent) >= 0.0 {
        (end_angle - start_angle).rem_euclid(std::f64::consts::TAU)
    } else {
        -((start_angle - end_angle).rem_euclid(std::f64::consts::TAU))
    };
    let middle_angle = start_angle + sweep * 0.5;
    Some(center + DVec2::new(middle_angle.cos(), middle_angle.sin()) * signed_radius.abs())
}

fn arc_start_tangent(start: DVec2, mid: DVec2, end: DVec2) -> Option<DVec2> {
    let (center, _) = arc_center_radius(start, mid, end)?;
    let angle = |point: DVec2| (point.y - center.y).atan2(point.x - center.x);
    let start_angle = angle(start);
    let sweep = (angle(end) - start_angle).rem_euclid(std::f64::consts::TAU);
    let mid_sweep = (angle(mid) - start_angle).rem_euclid(std::f64::consts::TAU);
    let radial = (start - center).normalize_or_zero();
    let ccw = DVec2::new(-radial.y, radial.x);
    Some(if mid_sweep <= sweep { ccw } else { -ccw })
}

fn arc_contains(entity: &SketchEntity, point: DVec2) -> bool {
    let SketchEntity::Arc { start, end, mid } = entity else {
        return true;
    };
    let Some((center, _)) = arc_center_radius(*start, *mid, *end) else {
        return false;
    };
    arc_parameter(center, *start, *mid, *end, point).is_some()
}

fn arc_parameter(center: DVec2, start: DVec2, mid: DVec2, end: DVec2, point: DVec2) -> Option<f64> {
    let angle = |p: DVec2| (p.y - center.y).atan2(p.x - center.x);
    let a = angle(start);
    let mut sweep = (angle(end) - a).rem_euclid(std::f64::consts::TAU);
    if (angle(mid) - a).rem_euclid(std::f64::consts::TAU) > sweep {
        sweep -= std::f64::consts::TAU;
    }
    let candidate = if sweep >= 0.0 {
        (angle(point) - a).rem_euclid(std::f64::consts::TAU)
    } else {
        -(a - angle(point)).rem_euclid(std::f64::consts::TAU)
    };
    (candidate.abs() <= sweep.abs() + 1.0e-8).then_some(candidate / sweep)
}

fn deduplicate_points(points: &mut Vec<DVec2>) {
    let mut unique = Vec::new();
    for point in points.drain(..) {
        if !unique
            .iter()
            .any(|candidate: &DVec2| near(*candidate, point))
        {
            unique.push(point);
        }
    }
    *points = unique;
}

fn trim_line(a: DVec2, b: DVec2, pick: DVec2, points: &[DVec2]) -> Option<Vec<SketchEntity>> {
    let direction = b - a;
    let length_squared = direction.length_squared();
    let mut cuts = points
        .iter()
        .map(|point| (*point - a).dot(direction) / length_squared)
        .filter(|t| *t > ENDPOINT_EPSILON && *t < 1.0 - ENDPOINT_EPSILON)
        .collect::<Vec<_>>();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() <= ENDPOINT_EPSILON);
    if cuts.is_empty() {
        return None;
    }
    let pick_t = ((pick - a).dot(direction) / length_squared).clamp(0.0, 1.0);
    let mut bounds = vec![0.0];
    bounds.extend(cuts);
    bounds.push(1.0);
    let removed = bounds
        .windows(2)
        .position(|window| pick_t >= window[0] && pick_t <= window[1])?;
    Some(
        bounds
            .windows(2)
            .enumerate()
            .filter(|(index, _)| *index != removed)
            .map(|(_, interval)| SketchEntity::Line {
                a: a + direction * interval[0],
                b: a + direction * interval[1],
            })
            .collect(),
    )
}

fn removed_line(a: DVec2, b: DVec2, pick: DVec2, points: &[DVec2]) -> Option<SketchEntity> {
    let direction = b - a;
    let length_squared = direction.length_squared();
    let mut bounds = vec![0.0];
    bounds.extend(
        points
            .iter()
            .map(|point| (*point - a).dot(direction) / length_squared)
            .filter(|t| *t > ENDPOINT_EPSILON && *t < 1.0 - ENDPOINT_EPSILON),
    );
    bounds.sort_by(f64::total_cmp);
    bounds.dedup_by(|a, b| (*a - *b).abs() <= ENDPOINT_EPSILON);
    bounds.push(1.0);
    (bounds.len() > 2).then_some(())?;
    let pick_t = ((pick - a).dot(direction) / length_squared).clamp(0.0, 1.0);
    let interval = bounds
        .windows(2)
        .find(|window| pick_t >= window[0] && pick_t <= window[1])?;
    Some(SketchEntity::Line {
        a: a + direction * interval[0],
        b: a + direction * interval[1],
    })
}

fn trim_circle(
    center: DVec2,
    radius: f64,
    pick: DVec2,
    points: &[DVec2],
) -> Option<Vec<SketchEntity>> {
    let mut angles = points
        .iter()
        .map(|point| {
            (point.y - center.y)
                .atan2(point.x - center.x)
                .rem_euclid(std::f64::consts::TAU)
        })
        .collect::<Vec<_>>();
    angles.sort_by(f64::total_cmp);
    angles.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-8);
    if angles.len() < 2 {
        return None;
    }
    let pick_angle = (pick.y - center.y)
        .atan2(pick.x - center.x)
        .rem_euclid(std::f64::consts::TAU);
    let removed = (0..angles.len()).find(|&index| {
        let start = angles[index];
        let end = angles[(index + 1) % angles.len()]
            + if index + 1 == angles.len() {
                std::f64::consts::TAU
            } else {
                0.0
            };
        let pick = pick_angle
            + if pick_angle < start {
                std::f64::consts::TAU
            } else {
                0.0
            };
        pick >= start && pick <= end
    })?;
    let start_angle = angles[(removed + 1) % angles.len()];
    let end_angle = angles[removed];
    let sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
    Some(vec![arc_from_angles(center, radius, start_angle, sweep)])
}

fn removed_circle(
    center: DVec2,
    radius: f64,
    pick: DVec2,
    points: &[DVec2],
) -> Option<SketchEntity> {
    let mut angles = points
        .iter()
        .map(|point| {
            (point.y - center.y)
                .atan2(point.x - center.x)
                .rem_euclid(std::f64::consts::TAU)
        })
        .collect::<Vec<_>>();
    angles.sort_by(f64::total_cmp);
    angles.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-8);
    if angles.len() < 2 {
        return None;
    }
    let pick_angle = (pick.y - center.y)
        .atan2(pick.x - center.x)
        .rem_euclid(std::f64::consts::TAU);
    let index = (0..angles.len()).find(|&index| {
        let start = angles[index];
        let end = angles[(index + 1) % angles.len()]
            + if index + 1 == angles.len() {
                std::f64::consts::TAU
            } else {
                0.0
            };
        let pick = pick_angle
            + if pick_angle < start {
                std::f64::consts::TAU
            } else {
                0.0
            };
        pick >= start && pick <= end
    })?;
    let start = angles[index];
    let sweep = (angles[(index + 1) % angles.len()] - start).rem_euclid(std::f64::consts::TAU);
    Some(arc_from_angles(center, radius, start, sweep))
}

fn trim_arc(
    start: DVec2,
    mid: DVec2,
    end: DVec2,
    pick: DVec2,
    points: &[DVec2],
) -> Option<Vec<SketchEntity>> {
    let (center, radius) = arc_center_radius(start, mid, end)?;
    let mut cuts = points
        .iter()
        .filter_map(|point| arc_parameter(center, start, mid, end, *point))
        .filter(|t| *t > ENDPOINT_EPSILON && *t < 1.0 - ENDPOINT_EPSILON)
        .collect::<Vec<_>>();
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() <= ENDPOINT_EPSILON);
    if cuts.is_empty() {
        return None;
    }
    let pick_t = arc_parameter(center, start, mid, end, pick)?.clamp(0.0, 1.0);
    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let end_angle = (end.y - center.y).atan2(end.x - center.x);
    let mut sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
    let mid_angle = (mid.y - center.y).atan2(mid.x - center.x);
    if (mid_angle - start_angle).rem_euclid(std::f64::consts::TAU) > sweep {
        sweep -= std::f64::consts::TAU;
    }
    let mut bounds = vec![0.0];
    bounds.extend(cuts);
    bounds.push(1.0);
    let removed = bounds
        .windows(2)
        .position(|w| pick_t >= w[0] && pick_t <= w[1])?;
    Some(
        bounds
            .windows(2)
            .enumerate()
            .filter(|(i, _)| *i != removed)
            .map(|(_, w)| {
                arc_from_angles(
                    center,
                    radius,
                    start_angle + sweep * w[0],
                    sweep * (w[1] - w[0]),
                )
            })
            .collect(),
    )
}

fn removed_arc(
    start: DVec2,
    mid: DVec2,
    end: DVec2,
    pick: DVec2,
    points: &[DVec2],
) -> Option<SketchEntity> {
    let (center, radius) = arc_center_radius(start, mid, end)?;
    let mut bounds = vec![0.0];
    bounds.extend(
        points
            .iter()
            .filter_map(|point| arc_parameter(center, start, mid, end, *point))
            .filter(|t| *t > ENDPOINT_EPSILON && *t < 1.0 - ENDPOINT_EPSILON),
    );
    bounds.sort_by(f64::total_cmp);
    bounds.dedup_by(|a, b| (*a - *b).abs() <= ENDPOINT_EPSILON);
    bounds.push(1.0);
    (bounds.len() > 2).then_some(())?;
    let pick_t = arc_parameter(center, start, mid, end, pick)?.clamp(0.0, 1.0);
    let interval = bounds
        .windows(2)
        .find(|window| pick_t >= window[0] && pick_t <= window[1])?;
    let start_angle = (start.y - center.y).atan2(start.x - center.x);
    let end_angle = (end.y - center.y).atan2(end.x - center.x);
    let mut sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
    let mid_angle = (mid.y - center.y).atan2(mid.x - center.x);
    if (mid_angle - start_angle).rem_euclid(std::f64::consts::TAU) > sweep {
        sweep -= std::f64::consts::TAU;
    }
    Some(arc_from_angles(
        center,
        radius,
        start_angle + sweep * interval[0],
        sweep * (interval[1] - interval[0]),
    ))
}

fn arc_from_angles(center: DVec2, radius: f64, start: f64, sweep: f64) -> SketchEntity {
    let point = |angle: f64| center + DVec2::new(angle.cos(), angle.sin()) * radius;
    SketchEntity::Arc {
        start: point(start),
        mid: point(start + sweep * 0.5),
        end: point(start + sweep),
    }
}

fn offset_polygon(points: &[DVec2], distance: f64) -> Option<Vec<SketchEntity>> {
    if points.len() < 3 {
        return None;
    }
    let area: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a.perp_dot(*b))
        .sum();
    let side = if area >= 0.0 { 1.0 } else { -1.0 };
    let shifted = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| {
            let direction = (*b - *a).normalize_or_zero();
            let outward = DVec2::new(direction.y, -direction.x) * side * distance;
            (*a + outward, *b + outward)
        })
        .collect::<Vec<_>>();
    let corners = (0..points.len())
        .map(|index| {
            let previous = shifted[(index + points.len() - 1) % points.len()];
            let current = shifted[index];
            infinite_line_intersection(previous.0, previous.1, current.0, current.1)
        })
        .collect::<Option<Vec<_>>>()?;
    Some(
        (0..corners.len())
            .map(|index| SketchEntity::Line {
                a: corners[index],
                b: corners[(index + 1) % corners.len()],
            })
            .collect(),
    )
}

fn offset_curve_loop(
    entities: &[SketchEntity],
    curves: &[(usize, bool)],
    distance: f64,
) -> Option<Vec<SketchEntity>> {
    let sampled = curves
        .iter()
        .flat_map(|(index, reversed)| {
            let mut points = match entities.get(*index) {
                Some(SketchEntity::Line { a, b }) => vec![*a, *b],
                Some(SketchEntity::Arc { start, end, mid }) => sample_arc(*start, *mid, *end, 16),
                _ => Vec::new(),
            };
            if *reversed {
                points.reverse();
            }
            points
        })
        .collect::<Vec<_>>();
    let area: f64 = sampled
        .iter()
        .zip(sampled.iter().cycle().skip(1))
        .take(sampled.len())
        .map(|(a, b)| a.perp_dot(*b))
        .sum();
    let side = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut offset = curves
        .iter()
        .map(|(index, reversed)| match entities.get(*index)? {
            SketchEntity::Line { a, b } => {
                let (a, b) = if *reversed { (*b, *a) } else { (*a, *b) };
                let direction = (b - a).normalize_or_zero();
                let shift = DVec2::new(direction.y, -direction.x) * side * distance;
                Some(SketchEntity::Line {
                    a: a + shift,
                    b: b + shift,
                })
            }
            SketchEntity::Arc { start, end, mid } => {
                let (start, end) = if *reversed {
                    (*end, *start)
                } else {
                    (*start, *end)
                };
                let (center, radius) = arc_center_radius(start, *mid, end)?;
                let start_angle = (start.y - center.y).atan2(start.x - center.x);
                let end_angle = (end.y - center.y).atan2(end.x - center.x);
                let mid_angle = (mid.y - center.y).atan2(mid.x - center.x);
                let mut sweep = (end_angle - start_angle).rem_euclid(std::f64::consts::TAU);
                if (mid_angle - start_angle).rem_euclid(std::f64::consts::TAU) > sweep {
                    sweep -= std::f64::consts::TAU;
                }
                let radius = radius + side * distance * sweep.signum();
                (radius > ENDPOINT_EPSILON)
                    .then(|| arc_from_angles(center, radius, start_angle, sweep))
            }
            SketchEntity::Circle { .. } => None,
            SketchEntity::Spline { .. }
            | SketchEntity::CvSpline { .. }
            | SketchEntity::Ellipse { .. }
            | SketchEntity::EllipseArc { .. }
            | SketchEntity::Point { .. } => None,
        })
        .collect::<Option<Vec<_>>>()?;

    // Adjacent offset lines meet at their miter. Line/arc and arc/arc joins
    // already share the same displaced endpoint when the source loop is tangent.
    for index in 0..offset.len() {
        let next = (index + 1) % offset.len();
        let intersection = match (&offset[index], &offset[next]) {
            (SketchEntity::Line { a: a0, b: a1 }, SketchEntity::Line { a: b0, b: b1 }) => {
                infinite_line_intersection(*a0, *a1, *b0, *b1)
            }
            _ => None,
        };
        if let Some(point) = intersection {
            if let SketchEntity::Line { b, .. } = &mut offset[index] {
                *b = point;
            }
            if let SketchEntity::Line { a, .. } = &mut offset[next] {
                *a = point;
            }
        }
    }
    Some(offset)
}

fn infinite_line_intersection(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> Option<DVec2> {
    let ab = b - a;
    let cd = d - c;
    let denominator = ab.perp_dot(cd);
    (denominator.abs() > 1.0e-12).then(|| a + ab * ((c - a).perp_dot(cd) / denominator))
}

/// Returns evenly parameterized points along a three-point circular arc.
pub fn sample_arc(start: DVec2, mid: DVec2, end: DVec2, segments: usize) -> Vec<DVec2> {
    let Some((center, radius)) = arc_center_radius(start, mid, end) else {
        return vec![start, end];
    };
    let angle = |point: DVec2| (point.y - center.y).atan2(point.x - center.x);
    let a = angle(start);
    let mut sweep = (angle(end) - a).rem_euclid(std::f64::consts::TAU);
    let mid_sweep = (angle(mid) - a).rem_euclid(std::f64::consts::TAU);
    if mid_sweep > sweep {
        sweep -= std::f64::consts::TAU;
    }
    (0..=segments.max(1))
        .map(|index| {
            let theta = a + sweep * index as f64 / segments.max(1) as f64;
            center + DVec2::new(theta.cos(), theta.sin()) * radius
        })
        .collect()
}

/// Returns the circle center and radius of a non-collinear three-point arc.
pub fn arc_center_radius(start: DVec2, mid: DVec2, end: DVec2) -> Option<(DVec2, f64)> {
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

fn near(a: DVec2, b: DVec2) -> bool {
    a.distance_squared(b) <= ENDPOINT_EPSILON * ENDPOINT_EPSILON
}

/// Snaps a candidate segment horizontal or vertical when within `degrees`.
pub fn snap_horizontal_vertical(start: DVec2, candidate: DVec2, degrees: f64) -> (DVec2, bool) {
    let delta = candidate - start;
    if delta.length_squared() <= f64::EPSILON {
        return (candidate, false);
    }
    let threshold = degrees.to_radians().sin();
    if delta.y.abs() / delta.length() <= threshold {
        (DVec2::new(candidate.x, start.y), true)
    } else if delta.x.abs() / delta.length() <= threshold {
        (DVec2::new(start.x, candidate.y), true)
    } else {
        (candidate, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(closed: bool) -> Sketch {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        let points = [
            DVec2::new(0.0, 0.0),
            DVec2::new(40.0, 0.0),
            DVec2::new(40.0, 30.0),
            DVec2::new(0.0, 30.0),
        ];
        let count = if closed { 4 } else { 3 };
        for index in 0..count {
            sketch.entities.push(
                SketchEntity::Line {
                    a: points[index],
                    b: points[(index + 1) % points.len()],
                }
                .into(),
            );
        }
        sketch
    }

    #[test]
    fn rectangle_chain_closes_but_open_polyline_does_not() {
        assert_eq!(rectangle(true).profiles().len(), 1);
        assert!(rectangle(false).profiles().is_empty());
        assert_eq!(rectangle(false).open_chains(), vec![vec![0, 1, 2]]);
        assert!(rectangle(true).open_chains().is_empty());
    }

    #[test]
    fn circle_is_a_profile() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.push(
            SketchEntity::Circle {
                center: DVec2::ZERO,
                radius: 12.0,
            }
            .into(),
        );
        assert_eq!(sketch.profiles().len(), 1);
    }

    #[test]
    fn ellipse_profile_face_matches_semi_axes() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.push(
            SketchEntity::Ellipse {
                center: DVec2::new(3.0, 4.0),
                major: DVec2::new(20.0, 0.0),
                minor_ratio: 0.5,
            }
            .into(),
        );
        let face = sketch.to_face(&sketch.profiles()[0]).expect("ellipse face");
        let (minimum, maximum) = face.as_shape().aabb().expect("ellipse bounds");
        assert!(minimum.abs_diff_eq(DVec3::new(-17.0, -6.0, 0.0), 1.0e-5));
        assert!(maximum.abs_diff_eq(DVec3::new(23.0, 14.0, 0.0), 1.0e-5));
    }

    #[test]
    fn tangent_arc_starts_parallel_to_requested_tangent() {
        let start = DVec2::ZERO;
        let tangent = DVec2::X;
        let end = DVec2::new(10.0, 10.0);
        let mid = tangent_arc_mid(start, tangent, end).expect("tangent arc");
        let points = sample_arc(start, mid, end, 256);
        let chord_tangent = (points[1] - points[0]).normalize();
        assert!(chord_tangent.dot(tangent) > 0.9999);
    }

    #[test]
    fn plane_mapping_roundtrips() {
        let plane = SketchPlane {
            origin: DVec3::new(3.0, 4.0, 5.0),
            x_axis: DVec3::Y,
            y_axis: DVec3::Z,
        };
        let local = DVec2::new(7.0, -2.0);
        assert!(plane.to_local(plane.to_world(local)).distance(local) < 1.0e-9);
    }

    #[test]
    fn horizontal_vertical_snap_uses_three_degrees() {
        let (horizontal, snapped) =
            snap_horizontal_vertical(DVec2::ZERO, DVec2::new(10.0, 0.4), 3.0);
        assert!(snapped);
        assert_eq!(horizontal.y, 0.0);
        let (_, snapped) = snap_horizontal_vertical(DVec2::ZERO, DVec2::new(10.0, 0.6), 3.0);
        assert!(!snapped);
    }

    #[test]
    fn rectangle_face_has_expected_bounds() {
        let sketch = rectangle(true);
        let face = sketch.to_face(&sketch.profiles()[0]).expect("face");
        let (minimum, maximum) = face.as_shape().aabb().expect("bounds");
        assert!(minimum.distance(DVec3::ZERO) < 1.0e-6);
        assert!(maximum.distance(DVec3::new(40.0, 30.0, 0.0)) < 1.0e-6);
    }
    #[test]
    fn demo_scene_entities_yield_two_profiles() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        let points = [
            DVec2::new(-20.0, -15.0),
            DVec2::new(20.0, -15.0),
            DVec2::new(20.0, 15.0),
            DVec2::new(-20.0, 15.0),
        ];
        for index in 0..4 {
            sketch.entities.push(
                SketchEntity::Line {
                    a: points[index],
                    b: points[(index + 1) % 4],
                }
                .into(),
            );
        }
        sketch.entities.push(
            SketchEntity::Circle {
                center: DVec2::new(60.0, 0.0),
                radius: 12.0,
            }
            .into(),
        );
        let profiles = sketch.profiles();
        assert_eq!(profiles.len(), 2);
        for profile in &profiles {
            let face = sketch.to_face(profile).expect("face from profile");
            let mesh = face.as_shape().mesh(0.5).expect("mesh");
            assert!(!mesh.indices.is_empty());
        }
    }

    #[test]
    fn line_and_arc_chain_closes_into_a_face() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend(
            [
                SketchEntity::Arc {
                    start: DVec2::new(-10.0, 0.0),
                    end: DVec2::new(10.0, 0.0),
                    mid: DVec2::new(0.0, 10.0),
                },
                SketchEntity::Line {
                    a: DVec2::new(10.0, 0.0),
                    b: DVec2::new(-10.0, 0.0),
                },
            ]
            .into_iter()
            .map(SketchItem::regular),
        );
        let profiles = sketch.profiles();
        assert_eq!(profiles.len(), 1);
        let face = sketch.to_face(&profiles[0]).expect("arc profile face");
        assert!(face.as_shape().mesh(0.5).is_ok());
    }

    #[test]
    fn spline_and_line_chain_closes_into_a_face() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend(
            [
                SketchEntity::Spline {
                    points: vec![
                        DVec2::new(-10.0, 0.0),
                        DVec2::new(0.0, 10.0),
                        DVec2::new(10.0, 0.0),
                    ],
                },
                SketchEntity::Line {
                    a: DVec2::new(10.0, 0.0),
                    b: DVec2::new(-10.0, 0.0),
                },
            ]
            .into_iter()
            .map(SketchItem::regular),
        );
        let profiles = sketch.profiles();
        assert_eq!(profiles.len(), 1);
        let face = sketch.to_face(&profiles[0]).expect("spline profile face");
        assert!(face.as_shape().mesh(0.5).is_ok());
    }

    #[test]
    fn fillet_right_angle_corner_radius_five() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend(
            [
                SketchEntity::Line {
                    a: DVec2::ZERO,
                    b: DVec2::new(10.0, 0.0),
                },
                SketchEntity::Line {
                    a: DVec2::ZERO,
                    b: DVec2::new(0.0, 10.0),
                },
            ]
            .into_iter()
            .map(SketchItem::regular),
        );
        assert!(sketch.fillet_lines(0, 1, 5.0));
        let SketchEntity::Arc { start, end, mid } = sketch.entities[2].geo else {
            panic!("fillet arc")
        };
        assert!(start.distance(DVec2::new(5.0, 0.0)) < 1.0e-9);
        assert!(end.distance(DVec2::new(0.0, 5.0)) < 1.0e-9);
        let (center, radius) = arc_center_radius(start, mid, end).expect("arc circle");
        assert!(center.distance(DVec2::new(5.0, 5.0)) < 1.0e-9);
        assert!((radius - 5.0).abs() < 1.0e-9);
    }

    #[test]
    fn trim_splits_a_crossed_line_into_two() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend(
            [
                SketchEntity::Line {
                    a: DVec2::new(-10.0, 0.0),
                    b: DVec2::new(10.0, 0.0),
                },
                SketchEntity::Line {
                    a: DVec2::new(-2.0, -5.0),
                    b: DVec2::new(-2.0, 5.0),
                },
                SketchEntity::Line {
                    a: DVec2::new(2.0, -5.0),
                    b: DVec2::new(2.0, 5.0),
                },
            ]
            .into_iter()
            .map(SketchItem::regular),
        );
        assert!(sketch.trim(0, DVec2::ZERO));
        assert_eq!(sketch.entities.len(), 4);
        let horizontal = sketch
            .entities
            .iter()
            .filter(|entity| matches!(&entity.geo, SketchEntity::Line { a, b } if (a.y - b.y).abs() < 1.0e-9))
            .count();
        assert_eq!(horizontal, 2);
    }

    #[test]
    fn extend_meets_nearest_intersection_exactly() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend([
            SketchEntity::Line {
                a: DVec2::ZERO,
                b: DVec2::X,
            }
            .into(),
            SketchEntity::Line {
                a: DVec2::new(3.0, -2.0),
                b: DVec2::new(3.0, 2.0),
            }
            .into(),
        ]);
        assert!(sketch.extend(0, DVec2::X));
        assert!(
            matches!(sketch.entities[0].geo, SketchEntity::Line { b, .. } if b.distance(DVec2::new(3.0, 0.0)) < 1.0e-12)
        );
    }

    #[test]
    fn break_preserves_total_line_geometry() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.push(
            SketchEntity::Line {
                a: DVec2::ZERO,
                b: DVec2::new(10.0, 0.0),
            }
            .into(),
        );
        assert!(sketch.break_at(0, DVec2::new(4.0, 0.3)));
        let length: f64 = sketch
            .entities
            .iter()
            .map(|item| match item.geo {
                SketchEntity::Line { a, b } => a.distance(b),
                _ => 0.0,
            })
            .sum();
        assert!((length - 10.0).abs() < 1.0e-12);
    }

    #[test]
    fn two_tangent_circle_has_zero_line_residuals() {
        let (center, radius) = two_tangent_circle(
            DVec2::ZERO,
            DVec2::X,
            DVec2::ZERO,
            DVec2::Y,
            2.0,
            DVec2::splat(3.0),
        )
        .expect("quadrant circle");
        assert!((center.y.abs() - radius).abs() < 1.0e-12);
        assert!((center.x.abs() - radius).abs() < 1.0e-12);
    }

    #[test]
    fn cv_spline_is_clamped_to_first_and_last_poles() {
        let control = vec![
            DVec2::ZERO,
            DVec2::new(2.0, 4.0),
            DVec2::new(6.0, 4.0),
            DVec2::new(8.0, 0.0),
        ];
        let sampled = sample_cv_spline(&control, 3, SketchPlane::xy());
        assert!(sampled.first().unwrap().distance(control[0]) < 1.0e-9);
        assert!(sampled.last().unwrap().distance(*control.last().unwrap()) < 1.0e-9);
    }

    #[test]
    fn offset_square_outward_by_five_grows_bbox_by_ten() {
        let mut sketch = rectangle(true);
        assert!(sketch.offset_profile(0, 5.0));
        let (minimum, maximum) = sketch.entities[4..].iter().fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(minimum, maximum), entity| {
                let SketchEntity::Line { a, b } = &entity.geo else {
                    return (minimum, maximum);
                };
                (minimum.min(*a).min(*b), maximum.max(*a).max(*b))
            },
        );
        assert!(minimum.distance(DVec2::new(-5.0, -5.0)) < 1.0e-9);
        assert!(maximum.distance(DVec2::new(45.0, 35.0)) < 1.0e-9);
    }
    #[test]
    fn sphere_from_face_returns_none_without_aborting() {
        let shape = occt::Shape::sphere(DVec3::new(0.0, 0.0, 30.0), 30.0).unwrap();
        for index in 0..shape.face_count().unwrap() as u32 {
            assert!(SketchPlane::from_face(&shape, index).is_none());
        }
    }

    #[test]
    fn regular_hexagon_closes_and_builds_face() {
        let (entities, constraints) =
            regular_polygon(DVec2::ZERO, DVec2::new(10.0, 0.0), 6).unwrap();
        assert_eq!(entities.len(), 6);
        assert_eq!(constraints.len(), 6);
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch
            .entities
            .extend(entities.into_iter().map(SketchItem::regular));
        let face = sketch.to_face(&sketch.profiles()[0]).expect("hexagon face");
        let mesh = face.as_shape().mesh(0.25).expect("hexagon mesh");
        let area: f64 = mesh
            .indices
            .chunks_exact(3)
            .map(|triangle| {
                let a = mesh.positions[triangle[0] as usize];
                let b = mesh.positions[triangle[1] as usize];
                let c = mesh.positions[triangle[2] as usize];
                (b - a).cross(c - a).length() * 0.5
            })
            .sum();
        assert!((area - 150.0 * 3.0_f64.sqrt()).abs() < 1.0e-4);
    }

    #[test]
    fn slot_profile_closes_and_builds_face() {
        let (entities, _) = slot(DVec2::ZERO, DVec2::new(30.0, 0.0), 5.0).unwrap();
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch
            .entities
            .extend(entities.into_iter().map(SketchItem::regular));
        assert_eq!(sketch.profiles().len(), 1);
        assert!(sketch.to_face(&sketch.profiles()[0]).is_some());
    }

    #[test]
    fn rounded_rectangle_tangencies_solve_and_report_dof() {
        let (mut entities, constraints) =
            rounded_rectangle(DVec2::ZERO, DVec2::new(40.0, 20.0), 4.0).unwrap();
        let result = crate::constraint::solve(&mut entities, &constraints, &[]);
        assert!(result.converged);
        assert!(result.dof > 0);
    }

    #[test]
    fn three_point_circle_matches_known_triangle_and_rejects_collinear() {
        let (center, radius) =
            three_point_circle(DVec2::ZERO, DVec2::new(4.0, 0.0), DVec2::new(0.0, 3.0)).unwrap();
        assert!(center.distance(DVec2::new(2.0, 1.5)) < 1.0e-12);
        assert!((radius - 2.5).abs() < 1.0e-12);
        assert!(three_point_circle(DVec2::ZERO, DVec2::X, DVec2::new(2.0, 0.0)).is_none());
    }

    #[test]
    fn ellipse_arc_and_line_close_into_face() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend(
            [
                SketchEntity::EllipseArc {
                    center: DVec2::ZERO,
                    major: DVec2::new(10.0, 0.0),
                    minor_ratio: 0.5,
                    start_angle: 0.0,
                    end_angle: std::f64::consts::PI,
                },
                SketchEntity::Line {
                    a: DVec2::new(-10.0, 0.0),
                    b: DVec2::new(10.0, 0.0),
                },
            ]
            .into_iter()
            .map(SketchItem::regular),
        );
        assert_eq!(sketch.profiles().len(), 1);
        assert!(sketch.to_face(&sketch.profiles()[0]).is_some());
    }

    #[test]
    fn construction_geometry_is_counted_but_excluded_from_profiles() {
        let mut sketch = rectangle(true);
        sketch
            .entities
            .push(SketchItem::construction(SketchEntity::Circle {
                center: DVec2::ZERO,
                radius: 2.0,
            }));
        assert_eq!(sketch.entities.len(), 5);
        assert_eq!(sketch.profiles().len(), 1);
        assert!(sketch.open_chains().is_empty());
    }

    #[test]
    fn w1_bare_entity_json_loads_as_non_construction() {
        let fixture = r#"{"id":1,"plane":{"origin":[0.0,0.0,0.0],"x_axis":[1.0,0.0,0.0],"y_axis":[0.0,1.0,0.0]},"entities":[{"t":"Line","v":{"a":[0.0,0.0],"b":[1.0,0.0]}}],"constraints":[],"pinned":[],"visible":true,"support_body":null}"#;
        let sketch: Sketch = serde_json::from_str(fixture).expect("W1 sketch JSON");
        assert_eq!(sketch.entities.len(), 1);
        assert!(!sketch.entities[0].construction);
        assert!(matches!(sketch.entities[0].geo, SketchEntity::Line { .. }));
    }

    #[test]
    fn sketch_mirror_preserves_flags_and_adds_endpoint_symmetry() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.extend([
            SketchItem::construction(SketchEntity::Line {
                a: DVec2::new(2.0, 1.0),
                b: DVec2::new(4.0, 3.0),
            }),
            SketchItem::regular(SketchEntity::Line {
                a: DVec2::new(0.0, -5.0),
                b: DVec2::new(0.0, 5.0),
            }),
        ]);
        assert!(sketch.mirror_entities(&[0], 1));
        assert!(sketch.entities[2].construction);
        let SketchEntity::Line { a, b } = sketch.entities[2].geo else {
            unreachable!()
        };
        assert!(a.distance(DVec2::new(-2.0, 1.0)) < 1.0e-12);
        assert!(b.distance(DVec2::new(-4.0, 3.0)) < 1.0e-12);
        assert_eq!(sketch.constraints.len(), 2);
        assert!(sketch.constraints.iter().all(|constraint| matches!(
            constraint,
            Constraint::Symmetric {
                axis: EntityRef(1),
                ..
            }
        )));
    }

    #[test]
    fn circular_pattern_count_four_has_quadrant_positions() {
        let mut sketch = Sketch::new(SketchId(1), SketchPlane::xy());
        sketch.entities.push(
            SketchEntity::Point {
                at: DVec2::new(10.0, 0.0),
            }
            .into(),
        );
        assert!(sketch.circular_pattern(&[0], DVec2::ZERO, 4));
        let points: Vec<_> = sketch
            .entities
            .iter()
            .map(|item| match item.geo {
                SketchEntity::Point { at } => at,
                _ => unreachable!(),
            })
            .collect();
        for expected in [
            DVec2::new(10.0, 0.0),
            DVec2::new(0.0, 10.0),
            DVec2::new(-10.0, 0.0),
            DVec2::new(0.0, -10.0),
        ] {
            assert!(points.iter().any(|point| point.distance(expected) < 1.0e-9));
        }
    }
}
