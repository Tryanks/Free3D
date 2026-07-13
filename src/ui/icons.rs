//! Original monochrome vector icons and their embedded [`AssetSource`].
//!
//! gpui renders an SVG as an alpha mask and tints it with the element's
//! `text_color`, so every icon here is a stroke-based `0 0 24 24` glyph whose
//! own colours are irrelevant. Icon bodies are stored inline (no separate
//! files) and wrapped in a shared `<svg>` envelope at load time; [`Assets`]
//! serves them to `svg().path("icons/<name>.svg")`.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Embedded asset source backing every `svg().path("icons/…")` in the chrome.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path
            .strip_prefix("icons/")
            .and_then(|rest| rest.strip_suffix(".svg"))
        else {
            return Ok(None);
        };
        Ok(icon_body(name).map(|body| Cow::Owned(wrap(body).into_bytes())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// Wraps an icon body in the shared stroke-styled SVG envelope.
fn wrap(body: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" \
         stroke=\"#000\" stroke-width=\"1.75\" stroke-linecap=\"round\" \
         stroke-linejoin=\"round\">{body}</svg>"
    )
}

/// Returns the inner SVG markup for `name`, if known.
fn icon_body(name: &str) -> Option<&'static str> {
    Some(match name {
        // Chrome / navigation
        "search" => r#"<circle cx="11" cy="11" r="7"/><path d="M16.5 16.5 21 21"/>"#,
        "home" => r#"<path d="M4 11l8-7 8 7"/><path d="M6 10v9h12v-9"/>"#,
        "undo" => r#"<path d="M9 7 4 12l5 5"/><path d="M4 12h11a5 5 0 0 1 5 5v1"/>"#,
        "redo" => r#"<path d="M15 7l5 5-5 5"/><path d="M20 12H9a5 5 0 0 0-5 5v1"/>"#,
        "import" => {
            r#"<path d="M12 4v11M8 11l4 4 4-4"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>"#
        }
        "export" => {
            r#"<path d="M12 15V4M8 8l4-4 4 4"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>"#
        }
        "settings" => {
            r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9 7 7M17 17l2.1 2.1M19.1 4.9 17 7M7 17l-2.1 2.1"/>"#
        }
        "sync" => {
            r#"<path d="M20 8a8 8 0 0 0-14-3L4 7"/><path d="M4 5v3h3"/><path d="M4 16a8 8 0 0 0 14 3l2-2"/><path d="M20 19v-3h-3"/>"#
        }
        "share" => {
            r#"<circle cx="6" cy="12" r="2.5"/><circle cx="18" cy="6" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="M8.2 10.9 15.8 7.1M8.2 13.1l7.6 3.8"/>"#
        }
        "help" => {
            r#"<circle cx="12" cy="12" r="9"/><path d="M9.2 9.2a2.8 2.8 0 0 1 5.4 1c0 1.9-2.6 2.3-2.6 4"/><path d="M12 17h.01"/>"#
        }

        // Tool groups
        "sketch" => r#"<path d="M4 20l4-1L19 8l-3-3L5 16l-1 4z"/><path d="M14 6l4 4"/>"#,
        "add" => r#"<path d="M12 5v14M5 12h14"/>"#,
        "transform" => {
            r#"<path d="M12 4v16M4 12h16M9 7l3-3 3 3M9 17l3 3 3-3M7 9l-3 3 3 3M17 9l3 3-3 3"/>"#
        }
        "modify" => {
            r#"<path d="M15 4a4 4 0 0 0-4.5 5.3l-6 6a1.8 1.8 0 0 0 2.5 2.5l6-6A4 4 0 0 0 20 8l-2.6 2.6L15 8.2 17.6 5.6A4 4 0 0 0 15 4z"/>"#
        }

        // Spaces (workspaces)
        "modeling" => {
            r#"<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z"/><path d="M12 12v9M12 12l8-4.5M12 12l-8-4.5"/>"#
        }
        "visualize" => {
            r#"<circle cx="12" cy="12" r="3"/><path d="M2 12s3.5-6.5 10-6.5S22 12 22 12"/>"#
        }
        "draw" => {
            r#"<path d="M4 20l3-1L18 8l-2-2L5 17l-1 3z"/><path d="M14 4l2 2"/><path d="M4 20h8"/>"#
        }

        // Right cluster
        "magnet" => {
            r#"<path d="M6 4v7a6 6 0 0 0 12 0V4"/><path d="M6 4h4v7a2 2 0 0 0 4 0V4h4"/><path d="M6 8h4M14 8h4"/>"#
        }
        "camera" => r#"<path d="M4 8h3l2-2h6l2 2h3v11H4z"/><circle cx="12" cy="13" r="3.5"/>"#,
        "display" => r#"<path d="M3 5h18v11H3z"/><path d="M8 20h8M12 16v4"/>"#,

        // Panels
        "items" => r#"<path d="M12 3l9 5-9 5-9-5 9-5z"/><path d="M3 13l9 5 9-5"/>"#,
        "history" => r#"<circle cx="12" cy="12" r="8"/><path d="M12 8v4l3 2"/>"#,
        "eye" => {
            r#"<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z"/><circle cx="12" cy="12" r="3"/>"#
        }
        "eye-off" => {
            r#"<path d="M3 3l18 18"/><path d="M10.6 6.2A9.7 9.7 0 0 1 12 6c6.5 0 10 6 10 6a17 17 0 0 1-3.3 3.9M6.5 7.7A16.6 16.6 0 0 0 2 12s3.5 6 10 6a9.4 9.4 0 0 0 3.5-.7"/>"#
        }
        "trash" => r#"<path d="M4 7h16M9 7V4h6v3M7 7l1 13h8l1-13M10 11v5M14 11v5"/>"#,

        // View cluster
        "views" => {
            r#"<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z"/><path d="M12 12v9M12 12l8-4.5M12 12l-8-4.5"/>"#
        }
        "grid" => r#"<path d="M4 4h16v16H4z"/><path d="M4 9h16M4 15h16M9 4v16M15 4v16"/>"#,
        "fov" => r#"<path d="M4 20 12 4l8 16"/><path d="M4 20h16" stroke-dasharray="2 2"/>"#,

        // Modes
        "section" => r#"<path d="M4 6h10v12H4z"/><path d="M14 6l6-3v12l-6 3"/>"#,
        "isolate" => r#"<circle cx="12" cy="12" r="4"/><path d="M12 3v3M12 18v3M3 12h3M18 12h3"/>"#,
        "measure" => {
            r#"<path d="M3 8l5-5 13 13-5 5L3 8z"/><path d="M7 8l1.5 1.5M10 5l2 2M13 8l1.5 1.5M8 13l2 2"/>"#
        }

        // Sketch tools
        "line" => {
            r#"<path d="M4 20 20 4"/><circle cx="4" cy="20" r="1.5"/><circle cx="20" cy="4" r="1.5"/>"#
        }
        "rectangle" => r#"<path d="M4 6h16v12H4z"/>"#,
        "center-rectangle" => r#"<path d="M4 6h16v12H4z"/><path d="M9 12h6M12 9v6"/>"#,
        "rounded-rectangle" => r#"<rect x="3" y="6" width="18" height="12" rx="3"/>"#,
        "polygon" => r#"<path d="M12 3l8 5v8l-8 5-8-5V8z"/><circle cx="12" cy="12" r="1"/>"#,
        "slot" => r#"<path d="M7 7h10a5 5 0 0 1 0 10H7A5 5 0 0 1 7 7z"/>"#,
        "circle" => r#"<circle cx="12" cy="12" r="8"/>"#,
        "three-point-circle" => {
            r#"<circle cx="12" cy="12" r="8"/><circle cx="12" cy="4" r="1"/><circle cx="5" cy="16" r="1"/><circle cx="19" cy="16" r="1"/>"#
        }
        "ellipse" => r#"<ellipse cx="12" cy="12" rx="9" ry="5"/><path d="M12 12l7-3"/>"#,
        "arc" => {
            r#"<path d="M4 18a12 12 0 0 1 16 0"/><circle cx="4" cy="18" r="1.5"/><circle cx="20" cy="18" r="1.5"/>"#
        }
        "ellipse-arc" => {
            r#"<path d="M4 14c2-7 13-10 17-3"/><circle cx="4" cy="14" r="1"/><circle cx="21" cy="11" r="1"/>"#
        }
        "point" => r#"<circle cx="12" cy="12" r="2"/><path d="M12 5v4M12 15v4M5 12h4M15 12h4"/>"#,
        "tangent-arc" => {
            r#"<path d="M4 18h5"/><path d="M9 18a10 10 0 0 1 10-10"/><path d="M6 15l3 3-3 3"/>"#
        }
        "spline" => r#"<path d="M4 18c3-9 6 6 8-2s5-8 8 1"/>"#,
        "sketch-fillet" => {
            r#"<path d="M4 19h5a10 10 0 0 0 10-10V4"/><path d="M9 19a10 10 0 0 1 10-10"/>"#
        }
        "trim" => {
            r#"<path d="M4 6l16 12M4 18 20 6"/><circle cx="8" cy="9" r="2"/><circle cx="8" cy="15" r="2"/>"#
        }
        "sketch-offset" => {
            r#"<path d="M4 17V7h10"/><path d="M9 21h8a4 4 0 0 0 4-4V9"/><path d="M14 4l3 3-3 3"/>"#
        }

        // Add / primitives
        "box" => {
            r#"<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3z"/><path d="M12 12v9M12 12l8-4.5M12 12l-8-4.5"/>"#
        }
        "cylinder" => {
            r#"<ellipse cx="12" cy="6" rx="7" ry="3"/><path d="M5 6v12a7 3 0 0 0 14 0V6"/>"#
        }
        "sphere" => {
            r#"<circle cx="12" cy="12" r="8"/><ellipse cx="12" cy="12" rx="8" ry="3"/><path d="M12 4v16"/>"#
        }
        "cone" => r#"<path d="M12 3l7 15a7 3 0 0 1-14 0L12 3z"/>"#,
        "torus" => {
            r#"<ellipse cx="12" cy="12" rx="8" ry="5"/><ellipse cx="12" cy="12" rx="3" ry="1.5"/>"#
        }
        "plane" => r#"<path d="M3 9l7-3 11 3-7 3-11-3z"/><path d="M10 6v12"/>"#,
        "axis" => r#"<path d="M5 19V5M5 19h14M5 19l10 3"/><path d="M5 5l-2 2M5 5l2 2"/>"#,

        // Transform tools
        "move" => {
            r#"<path d="M12 3v18M3 12h18M9 6l3-3 3 3M9 18l3 3 3-3M6 9l-3 3 3 3M18 9l3 3-3 3"/>"#
        }
        "translate" => r#"<path d="M5 19 19 5M19 5h-6M19 5v6"/><circle cx="5" cy="19" r="1.5"/>"#,
        "scale" => r#"<path d="M4 20l7-7M4 20v-6M4 20h6"/><path d="M20 4l-7 7M20 4v6M20 4h-6"/>"#,
        "mirror" => {
            r#"<path d="M12 3v18" stroke-dasharray="2 2"/><path d="M8 8 4 12l4 4z"/><path d="M16 8l4 4-4 4z"/>"#
        }
        "pattern" => {
            r##"<g fill="#000" stroke="none"><circle cx="7" cy="7" r="1.6"/><circle cx="12" cy="7" r="1.6"/><circle cx="17" cy="7" r="1.6"/><circle cx="7" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="17" cy="12" r="1.6"/><circle cx="7" cy="17" r="1.6"/><circle cx="12" cy="17" r="1.6"/><circle cx="17" cy="17" r="1.6"/></g>"##
        }
        "align" => r#"<path d="M4 4v16"/><path d="M8 8h9M8 12h6M8 16h9"/>"#,

        // Modify tools
        "extrude" => r#"<path d="M4 16l8-4 8 4-8 4-8-4z"/><path d="M12 12V3M9 6l3-3 3 3"/>"#,
        "revolve" => {
            r#"<path d="M6 4v16" stroke-dasharray="2 2"/><path d="M6 8c6 0 10 2 10 4s-4 4-10 4"/><path d="M13 14l3 2 1-3"/>"#
        }
        "sweep" => r#"<path d="M5 18C7 8 17 8 19 18"/><path d="M3 16h4v4H3z"/>"#,
        "loft" => r#"<path d="M7 4h4M5 20h8M7 4L5 20M11 4l2 16"/>"#,
        "patch" => {
            r#"<path d="M4 17c3-9 13-9 16 0"/><path d="M4 17h16"/><circle cx="4" cy="17" r="1"/><circle cx="20" cy="17" r="1"/>"#
        }
        "stitch" => {
            r#"<path d="M4 5h6v14H4zM14 5h6v14h-6z"/><path d="M10 8h4M10 12h4M10 16h4" stroke-dasharray="1 2"/>"#
        }
        "thicken" => {
            r#"<path d="M4 8h12v10H4zM8 4h12v10"/><path d="M4 8l4-4M16 8l4-4M16 18l4-4"/>"#
        }
        "delete-face" => r#"<path d="M4 6h16v12H4z"/><path d="M8 9l8 6M16 9l-8 6"/>"#,
        "exploded" => {
            r#"<path d="M9.5 9.5h5v5h-5z"/><path d="M12 6.5V3M10.2 4.6 12 2.8l1.8 1.8M12 17.5V21M10.2 19.4l1.8 1.8 1.8-1.8M6.5 12H3M4.6 10.2 2.8 12l1.8 1.8M17.5 12H21M19.4 10.2l1.8 1.8-1.8 1.8"/>"#
        }
        "shell" => r#"<path d="M4 5v15h16V5"/><path d="M8 5v11h8V5"/>"#,
        "fillet" => {
            r#"<path d="M5 19V10a5 5 0 0 1 5-5h9"/><path d="M5 5v14h14" stroke-dasharray="2 2"/>"#
        }
        "offset" => r#"<path d="M4 5h9v14H4z"/><path d="M16 12h5"/><path d="M18.8 9.8 21 12l-2.2 2.2"/>"#,
        "draft" => {
            r#"<path d="M4 20h16"/><path d="M7 20V6"/><path d="M7 20 17 5"/><path d="M7 11c1.8 0 3.4.7 4.6 1.9"/>"#
        }
        "replace-face" => {
            r#"<path d="M4 10v10h16V10"/><path d="M4 10h16" stroke-dasharray="2 2"/><path d="M4 6c5-3.4 11-3.4 16 0"/>"#
        }
        "split" => r#"<path d="M4 5h16v14H4z"/><path d="M12 5v14" stroke-dasharray="2 2"/>"#,
        "project" => r#"<path d="M12 3v9M9 9l3 3 3-3"/><ellipse cx="12" cy="18" rx="7" ry="2.5"/>"#,
        "union" => r#"<circle cx="9" cy="12" r="6"/><circle cx="15" cy="12" r="6"/>"#,
        "subtract" => {
            r#"<circle cx="9" cy="12" r="6"/><circle cx="15" cy="12" r="6" stroke-dasharray="2 2"/>"#
        }
        "intersect" => {
            r#"<circle cx="9" cy="12" r="6" stroke-dasharray="2 2"/><circle cx="15" cy="12" r="6" stroke-dasharray="2 2"/><path d="M12 7a6 6 0 0 1 0 10 6 6 0 0 1 0-10z"/>"#
        }

        _ => return None,
    })
}

/// Builds the asset path for a named icon.
pub fn path(name: &str) -> SharedString {
    SharedString::from(format!("icons/{name}.svg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_icons_resolve() {
        for name in ["search", "line", "extrude", "eye", "grid"] {
            assert!(icon_body(name).is_some(), "missing icon {name}");
        }
        assert!(icon_body("does-not-exist").is_none());
    }

    #[test]
    fn asset_source_wraps_body() {
        let bytes = Assets.load("icons/circle.svg").unwrap().unwrap();
        let svg = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<circle"));
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
    }
}
