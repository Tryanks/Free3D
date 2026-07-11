//! Visual design tokens for the floating chrome.
//!
//! A [`Theme`] carries every colour, radius, spacing and type size the UI
//! layer needs. Two variants ship: a clean near-white light theme (the
//! default) and an elegant desaturated blue-gray dark counterpart. Both share
//! one warm orange accent reserved for selection and active states. Spacing
//! follows a strict 4px grid.

use gpui::{BoxShadow, Hsla, Pixels, Rgba, hsla, point, px, rgba};

/// Converts a packed `0xRRGGBBAA` literal into an [`Hsla`] colour.
fn c(hex: u32) -> Hsla {
    let rgba: Rgba = rgba(hex);
    rgba.into()
}

/// The complete set of design tokens for one appearance.
#[derive(Clone, Debug)]
pub struct Theme {
    /// `true` for the dark variant.
    pub is_dark: bool,

    // Surfaces
    /// Translucent background for floating panels and the tool strip.
    pub panel: Hsla,
    /// Slightly brighter surface for popovers and menus.
    pub elevated: Hsla,
    /// Fill for inset wells (search field, cube placeholder).
    pub well: Hsla,

    // Borders
    /// Hairline border for panels and buttons.
    pub border: Hsla,
    /// Stronger border for focused or emphasised edges.
    pub border_strong: Hsla,

    // Text
    /// Primary foreground text and icon colour.
    pub text: Hsla,
    /// Secondary, lower-emphasis text.
    pub text_muted: Hsla,
    /// Disabled / tertiary text.
    pub text_faint: Hsla,

    // Interaction overlays (painted over a surface)
    /// Overlay tint for hovered controls.
    pub hover: Hsla,
    /// Overlay tint for pressed / selected controls.
    pub active: Hsla,

    // Accent
    /// Warm accent for active tools and selection.
    pub accent: Hsla,
    /// Brighter accent for accent-on-hover.
    pub accent_hover: Hsla,
    /// Low-alpha accent used as an active-state background wash.
    pub accent_wash: Hsla,
    /// Foreground colour to sit on top of a solid accent fill.
    pub on_accent: Hsla,

    // Signals
    /// Positive / X-axis hint colour.
    pub axis_x: Hsla,
    /// Y / secondary-axis hint colour.
    pub axis_y: Hsla,

    // Elevation
    /// Soft drop shadow for floating surfaces.
    pub shadow: Vec<BoxShadow>,

    // Radii (logical pixels)
    /// Panel / popover corner radius.
    pub radius_panel: f32,
    /// Button and chip corner radius.
    pub radius_control: f32,

    // Metrics (logical pixels)
    /// Standard square control size (buttons on the strip / top bar).
    pub control: f32,
    /// Icon glyph size inside a control.
    pub icon: f32,
    /// Base spacing unit; every gap is a multiple of this.
    pub unit: f32,

    // Typography (logical pixels)
    /// Title / brand type size.
    pub text_lg: f32,
    /// Default body type size.
    pub text_md: f32,
    /// Dense / secondary type size.
    pub text_sm: f32,

    /// Colours consumed by the wgpu viewport (linear-ish sRGB components).
    pub canvas: CanvasTheme,
}

/// The 3D canvas palette handed to the renderer as uniforms/tints.
///
/// Components are `[r, g, b, a]` in 0..=1 sRGB, matching the shader tint
/// convention already used by the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasTheme {
    /// Background gradient, top.
    pub bg_top: [f32; 4],
    /// Background gradient, bottom.
    pub bg_bottom: [f32; 4],
    /// Minor grid line colour (alpha = line strength).
    pub grid_minor: [f32; 4],
    /// Major grid line colour.
    pub grid_major: [f32; 4],
    /// X axis line colour.
    pub axis_x: [f32; 4],
    /// Y axis line colour.
    pub axis_y: [f32; 4],
    /// Base body material colour (pre-lighting).
    pub body: [f32; 4],
    /// BRep feature edge / silhouette line colour.
    pub edge: [f32; 4],
    /// Faint line colour for edges hidden behind shaded surfaces.
    pub hidden_edge: [f32; 4],
    /// Flat approximation colour for sectioned solid interiors.
    pub section_cap: [f32; 4],
    /// Committed sketch curve colour (under-defined state).
    pub sketch: [f32; 4],
    /// Fully-defined sketch curve colour.
    pub sketch_defined: [f32; 4],
    /// Construction sketch curve colour (dash substitute in the current line pipeline).
    pub sketch_construction: [f32; 4],
    /// Translucent profile fill.
    pub sketch_fill: [f32; 4],
    /// Orientation-cube face fill.
    pub cube_face: [f32; 4],
    /// Orientation-cube edge-chamfer fill.
    pub cube_chamfer: [f32; 4],
    /// Orientation-cube corner and label colour.
    pub cube_edge: [f32; 4],
    /// Orientation-cube hovered-region fill.
    pub cube_hover: [f32; 4],
}

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

impl Theme {
    /// The elegant dark theme.
    pub fn dark() -> Self {
        Theme {
            is_dark: true,
            panel: c(0x1a2230_f2),
            elevated: c(0x222c3c_f7),
            well: c(0x0f1520_cc),
            border: c(0xffffff_14),
            border_strong: c(0xffffff_2b),
            text: c(0xe7ecf4_ff),
            text_muted: c(0x9ba7ba_ff),
            text_faint: c(0x637084_ff),
            hover: c(0xffffff_12),
            active: c(0xffffff_1f),
            accent: c(0xff7a2f_ff),
            accent_hover: c(0xff8f4d_ff),
            accent_wash: c(0xff7a2f_29),
            on_accent: c(0x1a1206_ff),
            axis_x: c(0xe5533f_ff),
            axis_y: c(0x4f9dde_ff),
            shadow: soft_shadow(0x00000073),
            radius_panel: 10.0,
            radius_control: 8.0,
            control: 34.0,
            icon: 18.0,
            unit: 4.0,
            text_lg: 15.0,
            text_md: 13.0,
            text_sm: 11.0,
            canvas: CanvasTheme {
                bg_top: [0.070, 0.094, 0.125, 1.0],
                bg_bottom: [0.035, 0.048, 0.070, 1.0],
                grid_minor: [0.28, 0.32, 0.37, 0.16],
                grid_major: [0.34, 0.39, 0.45, 0.28],
                axis_x: [0.90, 0.33, 0.25, 0.55],
                axis_y: [0.31, 0.62, 0.87, 0.55],
                body: [0.73, 0.76, 0.79, 1.0],
                edge: [0.055, 0.065, 0.075, 0.94],
                hidden_edge: [0.48, 0.55, 0.64, 0.22],
                section_cap: [0.48, 0.45, 0.43, 1.0],
                sketch: [0.25, 0.72, 1.0, 1.0],
                sketch_defined: [0.35, 0.85, 0.45, 1.0],
                sketch_construction: [0.52, 0.64, 0.76, 1.0],
                sketch_fill: [0.16, 0.52, 0.86, 0.20],
                cube_face: [0.69, 0.72, 0.76, 1.0],
                cube_chamfer: [0.48, 0.52, 0.57, 1.0],
                cube_edge: [0.38, 0.42, 0.47, 1.0],
                cube_hover: [1.0, 0.43, 0.20, 1.0],
            },
        }
    }

    /// The default clean light theme.
    pub fn light() -> Self {
        Theme {
            is_dark: false,
            panel: c(0xf4f6fa_f2),
            elevated: c(0xffffff_f7),
            well: c(0xe7ecf3_e0),
            border: c(0x1420350f),
            border_strong: c(0x14203524),
            text: c(0x1b2431_ff),
            text_muted: c(0x5a6678_ff),
            text_faint: c(0x93a0b2_ff),
            hover: c(0x14203508),
            active: c(0x14203514),
            accent: c(0xf26a1f_ff),
            accent_hover: c(0xff8236_ff),
            accent_wash: c(0xf26a1f_24),
            on_accent: c(0xffffff_ff),
            axis_x: c(0xd2452f_ff),
            axis_y: c(0x2f83c7_ff),
            shadow: soft_shadow(0x1b243126),
            radius_panel: 10.0,
            radius_control: 8.0,
            control: 34.0,
            icon: 18.0,
            unit: 4.0,
            text_lg: 15.0,
            text_md: 13.0,
            text_sm: 11.0,
            canvas: CanvasTheme {
                bg_top: [0.952, 0.955, 0.960, 1.0],
                bg_bottom: [0.912, 0.917, 0.925, 1.0],
                grid_minor: [0.45, 0.47, 0.50, 0.18],
                grid_major: [0.38, 0.40, 0.44, 0.30],
                axis_x: [0.85, 0.27, 0.20, 0.60],
                axis_y: [0.22, 0.63, 0.32, 0.60],
                body: [0.66, 0.67, 0.69, 1.0],
                edge: [0.13, 0.14, 0.16, 0.96],
                hidden_edge: [0.30, 0.34, 0.40, 0.20],
                section_cap: [0.72, 0.68, 0.65, 1.0],
                sketch: [0.13, 0.45, 0.85, 1.0],
                sketch_defined: [0.13, 0.62, 0.30, 1.0],
                sketch_construction: [0.42, 0.55, 0.68, 1.0],
                sketch_fill: [0.13, 0.45, 0.85, 0.16],
                cube_face: [0.961, 0.965, 0.973, 1.0],
                cube_chamfer: [0.84, 0.86, 0.89, 1.0],
                cube_edge: [0.64, 0.68, 0.73, 1.0],
                cube_hover: [1.0, 0.72, 0.56, 1.0],
            },
        }
    }

    /// Multiplies the base spacing unit by `n` and returns pixels.
    pub fn space(&self, n: f32) -> Pixels {
        px(self.unit * n)
    }
}

/// Builds a two-part soft ambient + contact drop shadow.
fn soft_shadow(color_hex: u32) -> Vec<BoxShadow> {
    let color: Hsla = rgba(color_hex).into();
    vec![
        BoxShadow {
            color,
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(2.0),
            spread_radius: px(0.0),
            inset: false,
        },
        BoxShadow {
            color: hsla(color.h, color.s, color.l, color.a * 0.7),
            offset: point(px(0.0), px(12.0)),
            blur_radius: px(28.0),
            spread_radius: px(-6.0),
            inset: false,
        },
    ]
}
