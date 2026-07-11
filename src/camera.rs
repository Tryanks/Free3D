//! Z-up turntable camera and view-transition math.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use glam::{Mat4, Vec2, Vec3, Vec4};

// Keep a tiny non-zero horizontal component so `look_at_rh` retains a stable
// Z-up basis while allowing cube top/bottom picks to be effectively square-on.
const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.00001;

/// Camera state that frames one face square-on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceFrameTarget {
    /// Center of the face bounds.
    pub pivot: Vec3,
    /// Orbit yaw whose forward vector points opposite the face normal.
    pub yaw: f32,
    /// Orbit pitch whose forward vector points opposite the face normal.
    pub pitch: f32,
    /// Perspective distance that fills roughly 60% of the viewport.
    pub distance: f32,
}

/// Computes a square-on camera target for a face bounding box and normal.
pub fn frame_face_target(
    minimum: Vec3,
    maximum: Vec3,
    normal: Vec3,
    viewport_size: Vec2,
    vertical_fov: f32,
) -> Option<FaceFrameTarget> {
    if !minimum.is_finite() || !maximum.is_finite() || normal.length_squared() <= 1.0e-12 {
        return None;
    }
    let forward = -normal.normalize();
    let yaw = forward.y.atan2(forward.x);
    let pitch = forward.z.asin().clamp(-PITCH_LIMIT, PITCH_LIMIT);
    let right = forward.cross(Vec3::Z).normalize_or(Vec3::X);
    let up = right.cross(forward).normalize_or(Vec3::Y);
    let extent = (maximum - minimum).abs() * 0.5;
    let projected_half_width = extent.dot(right.abs());
    let projected_half_height = extent.dot(up.abs());
    let aspect = (viewport_size.x / viewport_size.y.max(1.0)).max(0.001);
    let tan_half_fov = (vertical_fov * 0.5).tan().max(1.0e-4);
    let fill = 0.6;
    let distance = (projected_half_height / (fill * tan_half_fov))
        .max(projected_half_width / (fill * tan_half_fov * aspect))
        .max(0.2);
    Some(FaceFrameTarget {
        pivot: (minimum + maximum) * 0.5,
        yaw,
        pitch,
        distance,
    })
}

/// A perspective, Z-up orbit camera with damped target values.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    pub pivot: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov: f32,
    pub viewport_size: Vec2,
    target_pivot: Vec3,
    target_yaw: f32,
    target_pitch: f32,
    target_distance: f32,
    transition_seconds: f32,
}

impl OrbitCamera {
    /// Creates the default front-right-top view around `pivot`.
    pub fn new(pivot: Vec3, distance: f32, viewport_size: Vec2) -> Self {
        let yaw = -3.0 * FRAC_PI_4;
        let pitch = -0.55;
        Self {
            pivot,
            yaw,
            pitch,
            distance,
            fov: FRAC_PI_4,
            viewport_size,
            target_pivot: pivot,
            target_yaw: yaw,
            target_pitch: pitch,
            target_distance: distance,
            transition_seconds: 0.15,
        }
    }

    /// Returns the camera position.
    pub fn eye(&self) -> Vec3 {
        self.pivot + self.forward_from_target() * -self.distance
    }

    fn forward_from_target(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
        )
    }

    /// World-to-view transform.
    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.pivot, Vec3::Z)
    }

    /// Perspective projection using wgpu's zero-to-one depth range.
    pub fn projection_matrix(&self) -> Mat4 {
        let aspect = (self.viewport_size.x / self.viewport_size.y.max(1.0)).max(0.001);
        Mat4::perspective_rh(self.fov, aspect, 0.1, 10_000.0)
    }

    /// Combined projection-view matrix.
    pub fn view_projection(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Adjusts the target orbit angles, clamping pitch short of the poles.
    pub fn orbit(&mut self, delta: Vec2) {
        self.target_yaw -= delta.x * 0.006;
        self.target_pitch = (self.target_pitch + delta.y * 0.006).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Moves the target pivot in camera screen space.
    pub fn pan(&mut self, delta: Vec2) {
        let forward = (self.target_pivot - self.target_eye()).normalize();
        let right = forward.cross(Vec3::Z).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let units_per_pixel =
            2.0 * self.target_distance * (self.fov * 0.5).tan() / self.viewport_size.y.max(1.0);
        self.target_pivot += (right * -delta.x + up * delta.y) * units_per_pixel;
    }

    fn target_eye(&self) -> Vec3 {
        let dir = Vec3::new(
            self.target_pitch.cos() * self.target_yaw.cos(),
            self.target_pitch.cos() * self.target_yaw.sin(),
            self.target_pitch.sin(),
        );
        self.target_pivot - dir * self.target_distance
    }

    /// Returns a world-space ray for a viewport pixel coordinate.
    pub fn unproject_ray(&self, pixel: Vec2) -> (Vec3, Vec3) {
        let ndc = Vec2::new(
            pixel.x / self.viewport_size.x.max(1.0) * 2.0 - 1.0,
            1.0 - pixel.y / self.viewport_size.y.max(1.0) * 2.0,
        );
        let inv = self.view_projection().inverse();
        let near = inv * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;
        (near, (far - near).normalize())
    }

    /// Projects a world point to viewport pixels.
    pub fn project(&self, world: Vec3) -> Vec2 {
        let clip = self.view_projection() * world.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        Vec2::new(
            (ndc.x + 1.0) * 0.5 * self.viewport_size.x,
            (1.0 - ndc.y) * 0.5 * self.viewport_size.y,
        )
    }

    /// Exponentially dollies about a picked world point, preserving its screen position.
    pub fn zoom_toward_point(&mut self, point: Vec3, logarithmic_delta: f32) {
        let scale = logarithmic_delta.exp().clamp(0.05, 20.0);
        self.target_pivot = point + (self.target_pivot - point) * scale;
        self.target_distance = (self.target_distance * scale).clamp(0.2, 50_000.0);
    }

    /// Picks a point on the plane through the pivot, normal to the view ray.
    pub fn cursor_plane_point(&self, pixel: Vec2) -> Vec3 {
        let (origin, ray) = self.unproject_ray(pixel);
        let normal = (self.pivot - self.eye()).normalize();
        let t = (self.pivot - origin).dot(normal) / ray.dot(normal).max(1.0e-6);
        origin + ray * t
    }

    /// Starts an eased standard-view transition.
    pub fn animate_to(&mut self, yaw: f32, pitch: f32, distance: f32) {
        self.target_yaw = yaw;
        self.target_pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.target_distance = distance.max(0.2);
        self.transition_seconds = 0.25;
    }

    /// Starts an eased transition that also centers a new model-space pivot.
    pub fn animate_to_pivot(&mut self, pivot: Vec3, yaw: f32, pitch: f32, distance: f32) {
        self.target_pivot = pivot;
        self.animate_to(yaw, pitch, distance);
    }

    /// Advances damping by `dt`; returns whether another frame is needed.
    pub fn tick(&mut self, dt: f32) -> bool {
        let alpha = 1.0 - (-dt / self.transition_seconds.max(0.001)).exp();
        self.pivot = self.pivot.lerp(self.target_pivot, alpha);
        self.yaw += angle_delta(self.yaw, self.target_yaw) * alpha;
        self.pitch += (self.target_pitch - self.pitch) * alpha;
        self.distance += (self.target_distance - self.distance) * alpha;
        let active = self.pivot.distance_squared(self.target_pivot) > 1.0e-6
            || angle_delta(self.yaw, self.target_yaw).abs() > 1.0e-4
            || (self.pitch - self.target_pitch).abs() > 1.0e-4
            || (self.distance - self.target_distance).abs() > 1.0e-3;
        if !active {
            self.pivot = self.target_pivot;
            self.yaw = self.target_yaw;
            self.pitch = self.target_pitch;
            self.distance = self.target_distance;
        }
        active
    }
}

fn angle_delta(from: f32, to: f32) -> f32 {
    (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> OrbitCamera {
        OrbitCamera::new(Vec3::ZERO, 100.0, Vec2::new(1200.0, 800.0))
    }

    #[test]
    fn orbit_clamps_pitch() {
        let mut camera = camera();
        camera.orbit(Vec2::new(0.0, 100_000.0));
        camera.tick(10.0);
        assert!(camera.pitch <= PITCH_LIMIT);
    }

    #[test]
    fn unproject_project_roundtrip() {
        let camera = camera();
        let pixel = Vec2::new(317.0, 519.0);
        let (origin, ray) = camera.unproject_ray(pixel);
        let world = origin + ray * 40.0;
        assert!(
            camera.project(world).distance(pixel) < 0.1,
            "projected={:?}, expected={pixel:?}",
            camera.project(world)
        );
    }

    #[test]
    fn zoom_toward_point_preserves_projection() {
        let mut camera = camera();
        let pixel = Vec2::new(820.0, 330.0);
        let point = camera.cursor_plane_point(pixel);
        camera.zoom_toward_point(point, -0.7);
        camera.tick(10.0);
        assert!(
            camera.project(point).distance(pixel) < 0.1,
            "projected={:?}, expected={pixel:?}, point={point:?}",
            camera.project(point)
        );
    }

    #[test]
    fn face_framing_is_square_on_and_uses_sixty_percent_height() {
        let target = frame_face_target(
            Vec3::new(-5.0, -1.0, -10.0),
            Vec3::new(5.0, 1.0, 10.0),
            Vec3::X,
            Vec2::new(1200.0, 800.0),
            FRAC_PI_4,
        )
        .unwrap();
        let forward = Vec3::new(
            target.pitch.cos() * target.yaw.cos(),
            target.pitch.cos() * target.yaw.sin(),
            target.pitch.sin(),
        );
        assert!(forward.distance(Vec3::NEG_X) < 1.0e-5);
        assert_eq!(target.pivot, Vec3::ZERO);
        let visible_height = 2.0 * target.distance * (FRAC_PI_4 * 0.5).tan();
        assert!((20.0 / visible_height - 0.6).abs() < 1.0e-5);
    }
}
