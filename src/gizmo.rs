//! Move/rotate gizmo handles, analytic picking, and drag math.

use glam::Vec3;

/// One independently pickable part of the move/rotate gizmo.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Handle {
    /// Translate along world X.
    AxisX,
    /// Translate along world Y.
    AxisY,
    /// Translate along world Z.
    AxisZ,
    /// Rotate around world X.
    RingX,
    /// Rotate around world Y.
    RingY,
    /// Rotate around world Z.
    RingZ,
    /// Translate in the camera-facing plane.
    Center,
}

impl Handle {
    /// Returns the world axis associated with an axis or ring handle.
    pub fn axis(self) -> Option<Vec3> {
        match self {
            Self::AxisX | Self::RingX => Some(Vec3::X),
            Self::AxisY | Self::RingY => Some(Vec3::Y),
            Self::AxisZ | Self::RingZ => Some(Vec3::Z),
            Self::Center => None,
        }
    }

    /// Returns whether this is a rotation handle.
    pub fn is_ring(self) -> bool {
        matches!(self, Self::RingX | Self::RingY | Self::RingZ)
    }
}

/// Finds the gizmo part intersected by a world-space ray.
pub fn hit_test(ray: (Vec3, Vec3), pivot: Vec3, scale: f32) -> Option<Handle> {
    let scale = scale.max(1.0e-6);
    let origin = (ray.0 - pivot) / scale;
    let direction = ray.1.normalize_or_zero();

    if ray_sphere(origin, direction, Vec3::ZERO, 0.15).is_some() {
        return Some(Handle::Center);
    }

    for (handle, axis) in [
        (Handle::AxisX, Vec3::X),
        (Handle::AxisY, Vec3::Y),
        (Handle::AxisZ, Vec3::Z),
    ] {
        let (ray_t, axis_t, distance) = closest_ray_axis(origin, direction, axis);
        let radius = if axis_t > 0.74 { 0.20 } else { 0.11 };
        if ray_t >= 0.0 && (0.14..=1.03).contains(&axis_t) && distance <= radius {
            return Some(handle);
        }
    }

    for (handle, normal) in [
        (Handle::RingX, Vec3::X),
        (Handle::RingY, Vec3::Y),
        (Handle::RingZ, Vec3::Z),
    ] {
        if let Some(point) = ray_plane(origin, direction, Vec3::ZERO, normal)
            && (point.length() - 0.70).abs() <= 0.065
        {
            return Some(handle);
        }
    }
    None
}

/// Tests one arbitrary-axis arrow using the move-gizmo arrow proportions.
pub fn hit_test_axis(ray: (Vec3, Vec3), pivot: Vec3, axis: Vec3, scale: f32) -> bool {
    let scale = scale.max(1.0e-6);
    let origin = (ray.0 - pivot) / scale;
    let direction = ray.1.normalize_or_zero();
    let (_, axis_t, distance) = closest_ray_axis(origin, direction, axis.normalize_or_zero());
    let (ray_t, _, _) = closest_ray_axis(origin, direction, axis.normalize_or_zero());
    let radius = if axis_t > 0.74 { 0.20 } else { 0.11 };
    ray_t >= 0.0 && (0.14..=1.03).contains(&axis_t) && distance <= radius
}

/// Parameter on `axis_origin + axis * t` closest to a cursor ray.
pub fn axis_drag_parameter(
    ray_origin: Vec3,
    ray_direction: Vec3,
    axis_origin: Vec3,
    axis: Vec3,
) -> f32 {
    let relative = ray_origin - axis_origin;
    let ray = ray_direction.normalize_or_zero();
    let axis = axis.normalize_or_zero();
    let dot = ray.dot(axis);
    let denominator = 1.0 - dot * dot;
    if denominator.abs() < 1.0e-5 {
        return relative.dot(axis);
    }
    (relative.dot(axis) - relative.dot(ray) * dot) / denominator
}

/// Intersects a ray with a plane, rejecting parallel or behind-camera hits.
pub fn ray_plane(
    ray_origin: Vec3,
    ray_direction: Vec3,
    plane_point: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let denominator = ray_direction.dot(plane_normal);
    if denominator.abs() < 1.0e-5 {
        return None;
    }
    let t = (plane_point - ray_origin).dot(plane_normal) / denominator;
    (t >= 0.0).then_some(ray_origin + ray_direction * t)
}

/// Quantizes an angle to five-degree increments when snapping is enabled.
pub fn snap_angle(angle: f32, enabled: bool) -> f32 {
    if enabled {
        let step = 5.0_f32.to_radians();
        (angle / step).round() * step
    } else {
        angle
    }
}

fn closest_ray_axis(origin: Vec3, direction: Vec3, axis: Vec3) -> (f32, f32, f32) {
    let dot = direction.dot(axis);
    let denominator = 1.0 - dot * dot;
    if denominator.abs() < 1.0e-5 {
        let axis_t = origin.dot(axis);
        return (0.0, axis_t, (origin - axis * axis_t).length());
    }
    let ray_t = (dot * origin.dot(axis) - origin.dot(direction)) / denominator;
    let axis_t = origin.dot(axis) + dot * ray_t;
    let distance = (origin + direction * ray_t - axis * axis_t).length();
    (ray_t, axis_t, distance)
}

fn ray_sphere(origin: Vec3, direction: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let relative = origin - center;
    let half_b = relative.dot(direction);
    let discriminant = half_b * half_b - (relative.length_squared() - radius * radius);
    if discriminant < 0.0 {
        return None;
    }
    let near = -half_b - discriminant.sqrt();
    (near >= 0.0).then_some(near)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_arrow_from_known_ray() {
        let hit = hit_test((Vec3::new(0.65, 0.04, 2.0), -Vec3::Z), Vec3::ZERO, 1.0);
        assert_eq!(hit, Some(Handle::AxisX));
    }

    #[test]
    fn hits_rotation_ring() {
        let hit = hit_test((Vec3::new(0.495, 0.495, 2.0), -Vec3::Z), Vec3::ZERO, 1.0);
        assert_eq!(hit, Some(Handle::RingZ));
    }

    #[test]
    fn misses_gizmo() {
        let hit = hit_test((Vec3::new(2.0, 2.0, 2.0), -Vec3::Z), Vec3::ZERO, 1.0);
        assert_eq!(hit, None);
    }

    #[test]
    fn axis_drag_delta_uses_two_ray_parameters() {
        let start = axis_drag_parameter(Vec3::new(2.0, 0.0, 5.0), -Vec3::Z, Vec3::ZERO, Vec3::X);
        let end = axis_drag_parameter(Vec3::new(5.5, 0.0, 5.0), -Vec3::Z, Vec3::ZERO, Vec3::X);
        assert!((end - start - 3.5).abs() < 1.0e-5);
    }

    #[test]
    fn rotation_angle_snaps_to_five_degrees() {
        assert!((snap_angle(32.4_f32.to_radians(), true).to_degrees() - 30.0).abs() < 1.0e-4);
        assert!((snap_angle(32.6_f32.to_radians(), true).to_degrees() - 35.0).abs() < 1.0e-4);
    }
}
