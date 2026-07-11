//! Remappable navigation gesture preset.

use gpui::{Modifiers, MouseButton, ScrollDelta};
use serde::{Deserialize, Serialize};

/// Camera operation produced by an input gesture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavAction {
    Orbit,
    Pan,
    Zoom,
}

/// User-selectable navigation binding table.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum NavPreset {
    /// Trackpad scroll orbits and Shift-scroll pans.
    #[default]
    Free3dDefault,
    /// Trackpad scroll zooms and Shift-scroll pans.
    ZoomScroll,
    /// Blender-compatible mouse and trackpad navigation.
    Blender,
    /// Autodesk Fusion-style middle-button navigation.
    Fusion,
    /// SolidWorks-style middle-button navigation.
    SolidWorks,
    /// Pan-first classic trackpad navigation.
    TrackpadClassic,
}

impl NavPreset {
    /// Every preset in settings-menu order.
    pub const ALL: [Self; 6] = [
        Self::Free3dDefault,
        Self::ZoomScroll,
        Self::Blender,
        Self::Fusion,
        Self::SolidWorks,
        Self::TrackpadClassic,
    ];

    /// User-facing preset name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Free3dDefault => "Free3D 默认",
            Self::ZoomScroll => "滚动缩放",
            Self::Blender => "Blender 风格",
            Self::Fusion => "Fusion 风格",
            Self::SolidWorks => "SolidWorks 风格",
            Self::TrackpadClassic => "触控板经典",
        }
    }

    fn bindings(self) -> &'static [NavBinding] {
        match self {
            Self::Free3dDefault => DEFAULT_PRESET,
            Self::ZoomScroll => ZOOM_SCROLL_PRESET,
            Self::Blender => BLENDER_PRESET,
            Self::Fusion => FUSION_PRESET,
            Self::SolidWorks => SOLIDWORKS_PRESET,
            Self::TrackpadClassic => TRACKPAD_CLASSIC_PRESET,
        }
    }
}

/// Normalized gesture kind used by navigation presets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GestureKind {
    TrackpadScroll,
    MouseWheel,
    MouseDrag(MouseButton),
    Pinch,
}

/// One entry in a remappable navigation preset.
#[derive(Clone, Copy, Debug)]
pub struct NavBinding {
    pub gesture: GestureKind,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub action: NavAction,
}

/// The default Shapr3D-style navigation preset. This is the single binding table.
pub const DEFAULT_PRESET: &[NavBinding] = &[
    NavBinding {
        gesture: GestureKind::TrackpadScroll,
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Orbit,
    },
    NavBinding {
        gesture: GestureKind::TrackpadScroll,
        shift: true,
        control: false,
        alt: false,
        action: NavAction::Pan,
    },
    NavBinding {
        gesture: GestureKind::MouseWheel,
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Zoom,
    },
    NavBinding {
        gesture: GestureKind::MouseDrag(MouseButton::Right),
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Orbit,
    },
    NavBinding {
        gesture: GestureKind::MouseDrag(MouseButton::Middle),
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Pan,
    },
    NavBinding {
        gesture: GestureKind::Pinch,
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Zoom,
    },
];

/// Desktop-style preset where unmodified two-finger scrolling zooms.
pub const ZOOM_SCROLL_PRESET: &[NavBinding] = &[
    NavBinding {
        gesture: GestureKind::TrackpadScroll,
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Zoom,
    },
    NavBinding {
        gesture: GestureKind::TrackpadScroll,
        shift: true,
        control: false,
        alt: false,
        action: NavAction::Pan,
    },
    NavBinding {
        gesture: GestureKind::MouseWheel,
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Zoom,
    },
    NavBinding {
        gesture: GestureKind::MouseDrag(MouseButton::Right),
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Orbit,
    },
    NavBinding {
        gesture: GestureKind::MouseDrag(MouseButton::Middle),
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Pan,
    },
    NavBinding {
        gesture: GestureKind::Pinch,
        shift: false,
        control: false,
        alt: false,
        action: NavAction::Zoom,
    },
];

macro_rules! binding {
    ($gesture:expr, $shift:expr, $control:expr, $alt:expr, $action:expr) => {
        NavBinding {
            gesture: $gesture,
            shift: $shift,
            control: $control,
            alt: $alt,
            action: $action,
        }
    };
}

/// Blender navigation: middle orbit, Shift-middle pan, wheel zoom, trackpad orbit.
pub const BLENDER_PRESET: &[NavBinding] = &[
    binding!(
        GestureKind::MouseDrag(MouseButton::Middle),
        false,
        false,
        false,
        NavAction::Orbit
    ),
    binding!(
        GestureKind::MouseDrag(MouseButton::Middle),
        true,
        false,
        false,
        NavAction::Pan
    ),
    binding!(
        GestureKind::MouseWheel,
        false,
        false,
        false,
        NavAction::Zoom
    ),
    binding!(
        GestureKind::TrackpadScroll,
        false,
        false,
        false,
        NavAction::Orbit
    ),
    binding!(GestureKind::Pinch, false, false, false, NavAction::Zoom),
];

/// Fusion navigation: middle pan and Shift-middle orbit.
pub const FUSION_PRESET: &[NavBinding] = &[
    binding!(
        GestureKind::MouseDrag(MouseButton::Middle),
        false,
        false,
        false,
        NavAction::Pan
    ),
    binding!(
        GestureKind::MouseDrag(MouseButton::Middle),
        true,
        false,
        false,
        NavAction::Orbit
    ),
    binding!(
        GestureKind::MouseWheel,
        false,
        false,
        false,
        NavAction::Zoom
    ),
    binding!(GestureKind::Pinch, false, false, false, NavAction::Zoom),
];

/// SolidWorks navigation: middle orbit and Control-middle pan.
pub const SOLIDWORKS_PRESET: &[NavBinding] = &[
    binding!(
        GestureKind::MouseDrag(MouseButton::Middle),
        false,
        false,
        false,
        NavAction::Orbit
    ),
    binding!(
        GestureKind::MouseDrag(MouseButton::Middle),
        false,
        true,
        false,
        NavAction::Pan
    ),
    binding!(
        GestureKind::MouseWheel,
        false,
        false,
        false,
        NavAction::Zoom
    ),
    binding!(GestureKind::Pinch, false, false, false, NavAction::Zoom),
];

/// Classic trackpad navigation: two-finger pan and Alt-two-finger orbit.
pub const TRACKPAD_CLASSIC_PRESET: &[NavBinding] = &[
    binding!(
        GestureKind::TrackpadScroll,
        false,
        false,
        false,
        NavAction::Pan
    ),
    binding!(
        GestureKind::TrackpadScroll,
        false,
        false,
        true,
        NavAction::Orbit
    ),
    binding!(GestureKind::Pinch, false, false, false, NavAction::Zoom),
    binding!(
        GestureKind::MouseWheel,
        false,
        false,
        false,
        NavAction::Zoom
    ),
];

/// Resolves a normalized gesture and modifiers through the preset table.
pub fn resolve(
    preset: NavPreset,
    gesture: GestureKind,
    modifiers: &Modifiers,
) -> Option<NavAction> {
    preset
        .bindings()
        .iter()
        .find(|binding| {
            binding.gesture == gesture
                && binding.shift == modifiers.shift
                && binding.control == modifiers.control
                && binding.alt == modifiers.alt
        })
        .map(|binding| binding.action)
}

/// Classifies gpui's scroll variants without conflating mouse and trackpad input.
pub fn scroll_gesture(delta: &ScrollDelta) -> GestureKind {
    match delta {
        ScrollDelta::Pixels(_) => GestureKind::TrackpadScroll,
        ScrollDelta::Lines(_) => GestureKind::MouseWheel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trackpad_scroll_depends_on_preset() {
        let modifiers = Modifiers::default();
        assert_eq!(
            resolve(
                NavPreset::Free3dDefault,
                GestureKind::TrackpadScroll,
                &modifiers,
            ),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            resolve(
                NavPreset::ZoomScroll,
                GestureKind::TrackpadScroll,
                &modifiers,
            ),
            Some(NavAction::Zoom)
        );
    }

    #[test]
    fn added_preset_mapping_rows_resolve() {
        let middle = GestureKind::MouseDrag(MouseButton::Middle);
        let plain = Modifiers::default();
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        let control = Modifiers {
            control: true,
            ..Default::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            resolve(NavPreset::Blender, middle, &plain),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            resolve(NavPreset::Blender, middle, &shift),
            Some(NavAction::Pan)
        );
        assert_eq!(
            resolve(NavPreset::Fusion, middle, &plain),
            Some(NavAction::Pan)
        );
        assert_eq!(
            resolve(NavPreset::Fusion, middle, &shift),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            resolve(NavPreset::SolidWorks, middle, &plain),
            Some(NavAction::Orbit)
        );
        assert_eq!(
            resolve(NavPreset::SolidWorks, middle, &control),
            Some(NavAction::Pan)
        );
        assert_eq!(
            resolve(
                NavPreset::TrackpadClassic,
                GestureKind::TrackpadScroll,
                &plain
            ),
            Some(NavAction::Pan)
        );
        assert_eq!(
            resolve(
                NavPreset::TrackpadClassic,
                GestureKind::TrackpadScroll,
                &alt
            ),
            Some(NavAction::Orbit)
        );

        for preset in [
            NavPreset::Blender,
            NavPreset::Fusion,
            NavPreset::SolidWorks,
            NavPreset::TrackpadClassic,
        ] {
            for binding in preset.bindings() {
                let modifiers = Modifiers {
                    shift: binding.shift,
                    control: binding.control,
                    alt: binding.alt,
                    ..Default::default()
                };
                assert_eq!(
                    resolve(preset, binding.gesture, &modifiers),
                    Some(binding.action),
                    "failed mapping row in {}",
                    preset.label()
                );
            }
        }
    }
}
