//! Session-level saved camera view slots.

use glam::Vec3;

/// Camera values captured by a saved-view slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedView {
    pub pivot: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_degrees: f32,
}

/// Eight session-only view slots owned by the application root.
#[derive(Clone, Debug, Default)]
pub struct SavedViews {
    slots: [Option<SavedView>; 8],
}

impl SavedViews {
    /// Number of view slots exposed by the UI.
    pub const LEN: usize = 8;

    /// Returns the saved state in `index`, if it is filled.
    pub fn get(&self, index: usize) -> Option<SavedView> {
        self.slots.get(index).copied().flatten()
    }

    /// Stores `view` in `index`, replacing any previous state.
    pub fn store(&mut self, index: usize, view: SavedView) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = Some(view);
        }
    }

    /// Clears `index` and returns the state that was present.
    pub fn clear(&mut self, index: usize) -> Option<SavedView> {
        self.slots.get_mut(index).and_then(Option::take)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_store_recall_and_clear_preserve_camera_math() {
        let view = SavedView {
            pivot: Vec3::new(12.5, -4.0, 8.25),
            yaw: -1.25,
            pitch: 0.42,
            distance: 93.75,
            fov_degrees: 37.0,
        };
        let mut slots = SavedViews::default();
        slots.store(3, view);

        let recalled = slots.get(3).expect("stored view is recallable");
        assert!((recalled.pivot - view.pivot).length() < f32::EPSILON);
        assert!((recalled.yaw - view.yaw).abs() < f32::EPSILON);
        assert!((recalled.pitch - view.pitch).abs() < f32::EPSILON);
        assert!((recalled.distance - view.distance).abs() < f32::EPSILON);
        assert!((recalled.fov_degrees - view.fov_degrees).abs() < f32::EPSILON);
        assert_eq!(slots.clear(3), Some(view));
        assert_eq!(slots.get(3), None);
    }
}
