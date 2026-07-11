//! Orientation-cube geometry semantics, camera mapping, and screen-space picking.

use std::{fs, path::Path};

use ab_glyph::{Font, FontVec, PxScale, point};
use glam::{Mat4, Vec2, Vec3, Vec4};

/// Logical inset from the viewport's top-right corner.
pub const INSET: f32 = 24.0;
/// Logical side length of the orientation cube's render and input region.
pub const SIZE: f32 = 96.0;
/// Local coordinate at which a face region transitions to an edge region.
pub const FACE_ZONE: f32 = 0.62;
/// Width and height in pixels of one face-label atlas cell.
pub const LABEL_CELL_SIZE: u32 = 64;
/// Width in cells of the single-row face-label atlas.
pub const LABEL_CELL_COUNT: u32 = 6;
/// Width in pixels of the face-label atlas.
pub const LABEL_ATLAS_WIDTH: u32 = LABEL_CELL_SIZE * LABEL_CELL_COUNT;

/// One face's label, atlas cell, and screen-upright local axes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaceLabel {
    /// Face region carrying this label.
    pub region: Region,
    /// Chinese character rasterized into the atlas.
    pub glyph: char,
    /// Zero-based atlas cell.
    pub cell: u32,
    /// World direction that appears right in the face's standard view.
    pub right: [i8; 3],
    /// World direction that appears up in the face's standard view.
    pub up: [i8; 3],
}

/// Face-label table in atlas order: top, bottom, front, back, left, right.
pub const FACE_LABELS: [FaceLabel; 6] = [
    face_label([0, 0, 1], '顶', 0, [0, -1, 0], [1, 0, 0]),
    face_label([0, 0, -1], '底', 1, [0, -1, 0], [-1, 0, 0]),
    face_label([0, -1, 0], '前', 2, [1, 0, 0], [0, 0, 1]),
    face_label([0, 1, 0], '后', 3, [-1, 0, 0], [0, 0, 1]),
    face_label([-1, 0, 0], '左', 4, [0, -1, 0], [0, 0, 1]),
    face_label([1, 0, 0], '右', 5, [0, 1, 0], [0, 0, 1]),
];

const fn face_label(
    signs: [i8; 3],
    glyph: char,
    cell: u32,
    right: [i8; 3],
    up: [i8; 3],
) -> FaceLabel {
    FaceLabel {
        region: Region { signs },
        glyph,
        cell,
        right,
        up,
    }
}

/// Returns the label metadata for a face; chamfers and corners are unlabeled.
pub fn face_label_for(region: Region) -> Option<FaceLabel> {
    FACE_LABELS
        .iter()
        .copied()
        .find(|label| label.region == region)
}

/// Rasterizes all six face glyphs into a single-row grayscale atlas.
///
/// The first usable macOS Chinese system font wins. If no candidate can be
/// loaded, this logs once and returns `None`, leaving the cube unlabeled.
pub fn build_label_atlas() -> Option<Vec<u8>> {
    const FONT_PATHS: [&str; 3] = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
    ];
    let font = FONT_PATHS
        .iter()
        .find_map(|path| load_font(Path::new(path)));
    let Some(font) = font else {
        eprintln!("orientation cube labels disabled: no supported macOS Chinese font found");
        return None;
    };

    let mut pixels = vec![0; (LABEL_ATLAS_WIDTH * LABEL_CELL_SIZE) as usize];
    for label in FACE_LABELS {
        rasterize_glyph(&font, label, &mut pixels);
    }
    Some(pixels)
}

fn load_font(path: &Path) -> Option<FontVec> {
    let bytes = fs::read(path).ok()?;
    FontVec::try_from_vec_and_index(bytes, 0).ok()
}

fn rasterize_glyph(font: &FontVec, label: FaceLabel, atlas: &mut [u8]) {
    let scale = PxScale::from(48.0);
    let glyph_id = font.glyph_id(label.glyph);
    let initial = font.outline_glyph(glyph_id.with_scale_and_position(scale, point(0.0, 0.0)));
    let Some(initial) = initial else {
        return;
    };
    let bounds = initial.px_bounds();
    let position = point(
        (LABEL_CELL_SIZE as f32 - bounds.width()) * 0.5 - bounds.min.x,
        (LABEL_CELL_SIZE as f32 - bounds.height()) * 0.5 - bounds.min.y,
    );
    let Some(outlined) = font.outline_glyph(glyph_id.with_scale_and_position(scale, position))
    else {
        return;
    };
    let bounds = outlined.px_bounds();
    outlined.draw(|x, y, coverage| {
        let atlas_x = label.cell * LABEL_CELL_SIZE + bounds.min.x.max(0.0) as u32 + x;
        let atlas_y = bounds.min.y.max(0.0) as u32 + y;
        if atlas_x < LABEL_ATLAS_WIDTH && atlas_y < LABEL_CELL_SIZE {
            atlas[(atlas_y * LABEL_ATLAS_WIDTH + atlas_x) as usize] =
                (coverage * 255.0).round() as u8;
        }
    });
}

/// One of the cube's six faces, twelve edges, or eight corners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    /// Signed outward axes: `-1`, `0`, or `1` for X, Y, and Z.
    pub signs: [i8; 3],
}

impl Region {
    /// All 26 regions in face, edge, then corner order.
    pub const ALL: [Self; 26] = [
        Self::new(-1, 0, 0),
        Self::new(1, 0, 0),
        Self::new(0, -1, 0),
        Self::new(0, 1, 0),
        Self::new(0, 0, -1),
        Self::new(0, 0, 1),
        Self::new(-1, -1, 0),
        Self::new(-1, 1, 0),
        Self::new(1, -1, 0),
        Self::new(1, 1, 0),
        Self::new(-1, 0, -1),
        Self::new(-1, 0, 1),
        Self::new(1, 0, -1),
        Self::new(1, 0, 1),
        Self::new(0, -1, -1),
        Self::new(0, -1, 1),
        Self::new(0, 1, -1),
        Self::new(0, 1, 1),
        Self::new(-1, -1, -1),
        Self::new(-1, -1, 1),
        Self::new(-1, 1, -1),
        Self::new(-1, 1, 1),
        Self::new(1, -1, -1),
        Self::new(1, -1, 1),
        Self::new(1, 1, -1),
        Self::new(1, 1, 1),
    ];

    const fn new(x: i8, y: i8, z: i8) -> Self {
        Self { signs: [x, y, z] }
    }

    /// Number of outward axes: one for faces, two for edges, three for corners.
    pub fn axis_count(self) -> usize {
        self.signs.iter().filter(|&&sign| sign != 0).count()
    }

    /// Unit outward direction represented by this region.
    pub fn direction(self) -> Vec3 {
        Vec3::new(
            f32::from(self.signs[0]),
            f32::from(self.signs[1]),
            f32::from(self.signs[2]),
        )
        .normalize()
    }
}

/// Device-pixel rectangle occupied by the cube.
#[derive(Clone, Copy, Debug)]
pub struct CubeRect {
    pub x: f32,
    pub y: f32,
    pub size: f32,
}

impl CubeRect {
    /// Whether a device-pixel pointer is within the cube's reserved input box.
    pub fn contains(self, pointer: Vec2) -> bool {
        pointer.x >= self.x
            && pointer.x <= self.x + self.size
            && pointer.y >= self.y
            && pointer.y <= self.y + self.size
    }
}

/// Returns the cube rectangle for a device-pixel viewport and backing scale.
pub fn cube_rect(width: u32, height: u32, scale: f32) -> CubeRect {
    let scale = scale.max(1.0);
    let size = SIZE * scale;
    let x = (width as f32 - (INSET * scale + size)).max(0.0);
    let y = (INSET * scale).min(height.saturating_sub(1) as f32);
    CubeRect {
        x,
        y,
        size: size.min(width as f32 - x).min(height as f32 - y).max(1.0),
    }
}

/// Classifies a unit-cube surface point as a face, edge, or corner region.
pub fn classify_hit(point: Vec3) -> Option<Region> {
    let mut signs = [0_i8; 3];
    for (index, coordinate) in point.to_array().into_iter().enumerate() {
        if coordinate.abs() > FACE_ZONE {
            signs[index] = if coordinate.is_sign_negative() { -1 } else { 1 };
        }
    }
    (signs != [0; 3]).then_some(Region { signs })
}

/// Converts an outward cube direction to orbit angles whose forward is `-direction`.
pub fn direction_to_orientation(direction: Vec3) -> (f32, f32) {
    let forward = -direction.normalize();
    (forward.y.atan2(forward.x), forward.z.asin())
}

/// Orthographic-ish projection used by both rendering and hit-testing.
pub fn view_projection(yaw: f32, pitch: f32) -> Mat4 {
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    Mat4::orthographic_rh(-1.65, 1.65, -1.65, 1.65, 0.1, 10.0)
        * Mat4::look_at_rh(-forward * 4.0, Vec3::ZERO, Vec3::Z)
}

/// Picks one of the 26 regions with a ray against the local unit cube.
pub fn pick_region(
    pointer: Vec2,
    viewport_width: u32,
    viewport_height: u32,
    scale: f32,
    yaw: f32,
    pitch: f32,
) -> Option<Region> {
    let rect = cube_rect(viewport_width, viewport_height, scale);
    if !rect.contains(pointer) {
        return None;
    }
    let ndc = Vec2::new(
        (pointer.x - rect.x) / rect.size * 2.0 - 1.0,
        1.0 - (pointer.y - rect.y) / rect.size * 2.0,
    );
    let inverse = view_projection(yaw, pitch).inverse();
    let near = inverse * Vec4::new(ndc.x, ndc.y, 0.0, 1.0);
    let far = inverse * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
    let near = near.truncate() / near.w;
    let far = far.truncate() / far.w;
    ray_unit_cube(near, (far - near).normalize()).and_then(classify_hit)
}

fn ray_unit_cube(origin: Vec3, direction: Vec3) -> Option<Vec3> {
    let mut near = f32::NEG_INFINITY;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        let origin_axis = origin[axis];
        let direction_axis = direction[axis];
        if direction_axis.abs() < 1.0e-7 {
            if origin_axis.abs() > 1.0 {
                return None;
            }
            continue;
        }
        let first = (-1.0 - origin_axis) / direction_axis;
        let second = (1.0 - origin_axis) / direction_axis;
        near = near.max(first.min(second));
        far = far.min(first.max(second));
    }
    (far >= near.max(0.0)).then(|| origin + direction * near.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::OrbitCamera;
    use crate::commands::StandardView;

    #[test]
    fn classifies_faces_edges_and_corners() {
        assert_eq!(
            classify_hit(Vec3::new(1.0, 0.2, -0.1))
                .unwrap()
                .axis_count(),
            1
        );
        assert_eq!(
            classify_hit(Vec3::new(1.0, -0.9, 0.1))
                .unwrap()
                .axis_count(),
            2
        );
        assert_eq!(
            classify_hit(Vec3::new(-1.0, 0.8, -0.7))
                .unwrap()
                .axis_count(),
            3
        );
        assert_eq!(
            classify_hit(Vec3::new(1.0, 0.62, 0.62)).unwrap().signs,
            [1, 0, 0]
        );
    }

    #[test]
    fn direction_orientation_roundtrips_faces_and_corner() {
        let directions = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
            Vec3::ONE.normalize(),
        ];
        for direction in directions {
            let (yaw, pitch) = direction_to_orientation(direction);
            let mut camera = OrbitCamera::new(Vec3::ZERO, 10.0, Vec2::splat(100.0));
            camera.animate_to(yaw, pitch, camera.distance);
            camera.tick(10.0);
            let forward = (camera.pivot - camera.eye()).normalize();
            assert!(
                forward.distance(-direction) < 1.0e-4,
                "direction={direction:?}, forward={forward:?}"
            );
        }
    }

    #[test]
    fn face_glyph_cells_match_standard_view_orientations() {
        let expected = [
            (StandardView::Top, [0, 0, 1], '顶', 0),
            (StandardView::Bottom, [0, 0, -1], '底', 1),
            (StandardView::Front, [0, -1, 0], '前', 2),
            (StandardView::Back, [0, 1, 0], '后', 3),
            (StandardView::Left, [-1, 0, 0], '左', 4),
            (StandardView::Right, [1, 0, 0], '右', 5),
        ];
        for (view, signs, glyph, cell) in expected {
            let label = face_label_for(Region { signs }).expect("face has a label");
            assert_eq!((label.glyph, label.cell), (glyph, cell));

            let (yaw, pitch) = view.orientation();
            let forward = Vec3::new(
                pitch.cos() * yaw.cos(),
                pitch.cos() * yaw.sin(),
                pitch.sin(),
            );
            assert!(
                forward.distance(-label.region.direction()) < 0.002,
                "{view:?} does not look toward the {glyph} face"
            );
        }
    }
}
