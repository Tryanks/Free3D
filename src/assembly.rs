//! Lightweight same-document assembly joints and deterministic chain solving.

use std::collections::{HashSet, VecDeque};

use glam::{DVec3, Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::{
    document::{AxisId, BodyId, Document, PlaneId, PointId},
    history::{EdgeRef, FaceRef, resolve_edge, resolve_face},
};

/// Stable identifier for an assembly joint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct JointId(pub u64);

/// A right-handed connector coordinate system stored in body-local coordinates.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConnectorFrame {
    pub origin: DVec3,
    pub z: DVec3,
    pub x: DVec3,
}

impl ConnectorFrame {
    /// Returns a sanitized orthonormal frame, choosing a stable X direction if needed.
    pub fn normalized(self) -> Self {
        let z = self.z.normalize_or_zero();
        let fallback = if z.x.abs() < 0.8 { DVec3::X } else { DVec3::Y };
        let mut x = (self.x - z * self.x.dot(z)).normalize_or_zero();
        if x.length_squared() < 0.5 {
            x = fallback.cross(z).normalize_or_zero();
        }
        Self {
            origin: self.origin,
            z,
            x,
        }
    }

    fn matrix(self) -> Mat4 {
        let frame = self.normalized();
        let x = frame.x.as_vec3();
        let z = frame.z.as_vec3();
        let y = z.cross(x).normalize_or_zero();
        Mat4::from_cols(
            x.extend(0.0),
            y.extend(0.0),
            z.extend(0.0),
            frame.origin.as_vec3().extend(1.0),
        )
    }
}

/// Replayable origin of a connector frame.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum ConnectorSource {
    Face(FaceRef),
    Edge(EdgeRef),
    Plane(PlaneId),
    Axis(AxisId),
    Point(PointId),
}

/// A derived connector together with its persistent source and stale status.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Connector {
    pub frame: ConnectorFrame,
    pub source: ConnectorSource,
    #[serde(default)]
    pub stale: bool,
}

/// Supported converged assembly joint types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum JointKind {
    Fixed,
    Revolute,
    Slider,
    Cylindrical,
    Ball,
}

/// A same-document relationship between connector frames on two bodies.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Joint {
    pub id: JointId,
    pub name: String,
    pub kind: JointKind,
    pub a: (BodyId, Connector),
    pub b: (BodyId, Connector),
    pub value: f64,
    pub value2: f64,
    pub limits: Option<(f64, f64)>,
}

fn stable_x(z: DVec3) -> DVec3 {
    let reference = if z.x.abs() < 0.8 { DVec3::X } else { DVec3::Y };
    reference.cross(z).normalize_or_zero()
}

impl Document {
    /// Re-derives a connector from current topology, preserving its stored fallback on failure.
    pub fn refresh_connector(&self, body: BodyId, connector: &mut Connector) {
        let derived = (|| -> Option<ConnectorFrame> {
            match &connector.source {
                ConnectorSource::Face(reference) => {
                    let shape = &self.bodies.iter().find(|item| item.id == body)?.shape;
                    let index = resolve_face(shape, reference).ok()? as usize;
                    if shape.face_surface_kind(index).ok()? != occt::SurfaceKind::Plane {
                        return None;
                    }
                    let origin = shape.face_center_of_mass(index).ok()?;
                    let z = shape
                        .face_normal_at(index, origin)
                        .ok()?
                        .normalize_or_zero();
                    Some(ConnectorFrame {
                        origin,
                        z,
                        x: stable_x(z),
                    })
                }
                ConnectorSource::Edge(reference) => {
                    let shape = &self.bodies.iter().find(|item| item.id == body)?.shape;
                    let index = resolve_edge(shape, reference).ok()? as usize;
                    let points = shape.edge_polyline(index, 0.02).ok()?;
                    if points.len() < 3 {
                        return None;
                    }
                    let origin = points.iter().copied().sum::<DVec3>() / points.len() as f64;
                    let mut normal = DVec3::ZERO;
                    for pair in points.windows(2) {
                        normal += (pair[0] - origin).cross(pair[1] - origin);
                    }
                    let z = normal.normalize_or_zero();
                    let radii: Vec<_> = points.iter().map(|point| point.distance(origin)).collect();
                    let mean = radii.iter().sum::<f64>() / radii.len() as f64;
                    if z.length_squared() < 0.5
                        || mean <= 1.0e-8
                        || radii
                            .iter()
                            .any(|radius| (radius - mean).abs() > mean * 0.08)
                    {
                        return None;
                    }
                    Some(ConnectorFrame {
                        origin,
                        z,
                        x: stable_x(z),
                    })
                }
                ConnectorSource::Plane(id) => self
                    .construction_planes
                    .iter()
                    .find(|item| item.id == *id)
                    .map(|item| ConnectorFrame {
                        origin: item.plane.origin,
                        z: item.plane.normal(),
                        x: item.plane.x_axis,
                    }),
                ConnectorSource::Axis(id) => self
                    .construction_axes
                    .iter()
                    .find(|item| item.id == *id)
                    .map(|item| ConnectorFrame {
                        origin: item.origin,
                        z: item.direction,
                        x: stable_x(item.direction),
                    }),
                ConnectorSource::Point(id) => self
                    .construction_points
                    .iter()
                    .find(|item| item.id == *id)
                    .map(|item| ConnectorFrame {
                        origin: item.position,
                        z: DVec3::Z,
                        x: DVec3::X,
                    }),
            }
        })();
        if let Some(frame) = derived {
            connector.frame = frame.normalized();
            connector.stale = false;
        } else {
            connector.stale = true;
        }
    }

    /// Solves all bodies reachable from grounded bodies using a deterministic BFS.
    pub fn solve_assembly(&mut self) {
        for body in &mut self.bodies {
            body.pose = Mat4::IDENTITY;
        }
        self.over_constrained = false;
        let mut posed: HashSet<_> = self.grounded.iter().copied().collect();
        let mut queue: VecDeque<_> = posed.iter().copied().collect();
        let mut used = HashSet::new();
        while let Some(current) = queue.pop_front() {
            for index in 0..self.joints.len() {
                if used.contains(&index) {
                    continue;
                }
                let (a_id, b_id) = (self.joints[index].a.0, self.joints[index].b.0);
                if current != a_id && current != b_id {
                    continue;
                }
                used.insert(index);
                let mut joint = self.joints[index].clone();
                self.refresh_connector(a_id, &mut joint.a.1);
                self.refresh_connector(b_id, &mut joint.b.1);
                if let Some((minimum, maximum)) = joint.limits {
                    joint.value = joint
                        .value
                        .clamp(minimum.min(maximum), minimum.max(maximum));
                }
                self.joints[index] = joint.clone();
                let (known, unknown, forward) = if current == a_id {
                    (a_id, b_id, true)
                } else {
                    (b_id, a_id, false)
                };
                if posed.contains(&unknown) {
                    self.over_constrained = true;
                    continue;
                }
                let known_pose = self
                    .bodies
                    .iter()
                    .find(|body| body.id == known)
                    .map_or(Mat4::IDENTITY, |body| body.pose);
                let dof = match joint.kind {
                    JointKind::Revolute => Mat4::from_rotation_z(joint.value as f32),
                    JointKind::Slider => Mat4::from_translation(Vec3::Z * joint.value as f32),
                    JointKind::Cylindrical => {
                        Mat4::from_translation(Vec3::Z * joint.value as f32)
                            * Mat4::from_rotation_z(joint.value2 as f32)
                    }
                    JointKind::Fixed | JointKind::Ball => Mat4::IDENTITY,
                };
                let fa = joint.a.1.frame.matrix();
                let fb = joint.b.1.frame.matrix();
                let pose = if forward {
                    known_pose * fa * dof * fb.inverse()
                } else {
                    known_pose * fb * dof.inverse() * fa.inverse()
                };
                if let Some(body) = self.bodies.iter_mut().find(|body| body.id == unknown) {
                    body.pose = pose;
                }
                posed.insert(unknown);
                queue.push_back(unknown);
            }
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
    }
}

/// Applies a runtime rigid pose to a read-only shape copy.
pub fn posed_shape(shape: &occt::Shape, pose: Mat4) -> Result<occt::Shape, String> {
    let (_, rotation, translation) = pose.to_scale_rotation_translation();
    let (axis, angle) = rotation.to_axis_angle();
    let rotated = if angle.abs() > f32::EPSILON {
        shape
            .rotated(DVec3::ZERO, axis.as_dvec3(), angle as f64)
            .map_err(|error| error.to_string())?
    } else {
        shape.clone()
    };
    rotated
        .translated(translation.as_dvec3())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history::PrimitiveKind, inspection::find_interferences};

    fn connector(origin: DVec3, z: DVec3) -> Connector {
        Connector {
            frame: ConnectorFrame {
                origin,
                z,
                x: stable_x(z),
            },
            source: ConnectorSource::Point(PointId(u64::MAX)),
            stale: false,
        }
    }

    fn joint(a: BodyId, b: BodyId, kind: JointKind, value: f64) -> Joint {
        Joint {
            id: JointId(0),
            name: "Joint".to_owned(),
            kind,
            a: (a, connector(DVec3::ZERO, DVec3::Z)),
            b: (b, connector(DVec3::ZERO, DVec3::Z)),
            value,
            value2: 0.0,
            limits: None,
        }
    }

    fn two_boxes() -> (Document, BodyId, BodyId) {
        let mut document = Document::new();
        let a = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        let b = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::new(2.0, 0.0, 0.0),
            max: DVec3::new(3.0, 1.0, 1.0),
        });
        (document, a, b)
    }

    #[test]
    fn revolute_aligns_frames_and_moves_marker_bbox() {
        let (mut document, base, arm) = two_boxes();
        document.set_grounded(base, true);
        document.add_joint(joint(
            base,
            arm,
            JointKind::Revolute,
            std::f64::consts::FRAC_PI_2,
        ));
        let posed = posed_shape(&document.bodies[1].shape, document.bodies[1].pose).unwrap();
        let (minimum, maximum) = posed.aabb().unwrap();
        assert!((minimum.x + 1.0).abs() < 1.0e-5);
        assert!((maximum.y - 3.0).abs() < 1.0e-5);
    }

    #[test]
    fn slider_translates_along_joint_axis_and_limits_clamp() {
        let (mut document, base, arm) = two_boxes();
        document.set_grounded(base, true);
        let mut slider = joint(base, arm, JointKind::Slider, 12.0);
        slider.limits = Some((-2.0, 5.0));
        document.add_joint(slider);
        assert_eq!(document.joints[0].value, 5.0);
        assert!((document.bodies[1].pose.transform_point3(Vec3::ZERO).z - 5.0).abs() < 1.0e-6);
    }

    #[test]
    fn two_joint_chain_solves_transitively_and_cycle_warns() {
        let (mut document, a, b) = two_boxes();
        let c = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::splat(4.0),
            max: DVec3::splat(5.0),
        });
        document.set_grounded(a, true);
        document.add_joint(joint(a, b, JointKind::Slider, 2.0));
        document.add_joint(joint(b, c, JointKind::Slider, 3.0));
        assert!((document.bodies[2].pose.transform_point3(Vec3::ZERO).z - 5.0).abs() < 1.0e-6);
        assert!(!document.over_constrained);
        document.add_joint(joint(c, a, JointKind::Fixed, 0.0));
        assert!(document.over_constrained);
    }

    #[test]
    fn grounding_joints_and_drive_roundtrip_and_replay() {
        let (mut document, base, arm) = two_boxes();
        document.set_grounded(base, true);
        let id = document
            .add_joint(joint(base, arm, JointKind::Revolute, 0.2))
            .unwrap();
        document.set_joint_value(id, 0.7, 0.0);
        let expected = document.bodies[1].pose;
        let path =
            std::env::temp_dir().join(format!("ductile-assembly-{}.ductile", std::process::id()));
        document.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        let _ = std::fs::remove_file(path);
        assert!(loaded.grounded.contains(&base));
        assert_eq!(loaded.joints, document.joints);
        assert!(loaded.bodies[1].pose.abs_diff_eq(expected, 1.0e-6));
        let replayed = crate::history::replay(&document.history).unwrap();
        assert!(replayed.bodies[1].pose.abs_diff_eq(expected, 1.0e-6));
    }

    #[test]
    fn posed_interference_finds_collision_absent_in_unposed_shapes() {
        let (mut document, base, arm) = two_boxes();
        assert!(
            find_interferences(&document.bodies, usize::MAX)
                .unwrap()
                .is_empty()
        );
        document.bodies[1].pose = Mat4::from_translation(Vec3::new(-2.0, 0.0, 0.0));
        assert_eq!(
            find_interferences(&document.bodies, usize::MAX)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(document.bodies[0].id, base);
        assert_eq!(document.bodies[1].id, arm);
    }
}
