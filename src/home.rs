//! Project-library discovery, file actions, thumbnails, and home-screen UI.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gpui::{ClickEvent, Context, FontWeight, ImageSource, RenderImage, div, img, prelude::*, px};
use image::{Frame, RgbaImage};
use smallvec::smallvec;

use crate::{app::Free3dApp, i18n::Lang, ui};

/// One project shown in the design library.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesignEntry {
    /// Native project path.
    pub path: PathBuf,
    /// Best-effort filesystem modification time.
    pub modified: SystemTime,
}

/// Returns the overridable, non-recursive design-library folder.
pub fn designs_dir() -> PathBuf {
    std::env::var_os("FREE3D_DESIGNS_DIR").map_or_else(
        || {
            dirs::document_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Free3D")
        },
        PathBuf::from,
    )
}

/// Builds the existing-file union of the design folder and recent projects.
pub fn list_designs_in(folder: &Path, recent: &[PathBuf]) -> Vec<DesignEntry> {
    let mut paths = Vec::new();
    if let Ok(entries) = std::fs::read_dir(folder) {
        paths.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("f3d"))
        }));
    }
    paths.extend(recent.iter().filter(|path| path.is_file()).cloned());
    let mut seen = HashSet::new();
    let mut designs: Vec<_> = paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .map(|path| DesignEntry {
            modified: std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH),
            path,
        })
        .collect();
    designs.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    designs
}

/// Lists projects from the configured design library and persisted recents.
pub fn list_designs(recent: &[PathBuf]) -> Vec<DesignEntry> {
    list_designs_in(&designs_dir(), recent)
}

/// Returns a collision-free duplicate path using localized copy suffixes.
pub fn unique_copy_path(source: &Path, language: Lang) -> PathBuf {
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Design");
    let extension = source.extension().and_then(|value| value.to_str());
    let suffix = if language == Lang::ZhCn {
        " 副本"
    } else {
        " Copy"
    };
    for number in 1.. {
        let name = if number == 1 {
            format!("{stem}{suffix}")
        } else {
            format!("{stem}{suffix} {number}")
        };
        let mut candidate = parent.join(name);
        if let Some(extension) = extension {
            candidate.set_extension(extension);
        }
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unused duplicate name exists")
}

/// Formats a compact relative time with deterministic English/Chinese copy.
pub fn relative_time(age: Duration, language: Lang) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        return crate::i18n::translate_for(language, "Just now").to_owned();
    }
    let (amount, singular, plural) = if seconds < 3_600 {
        (seconds / 60, "{} minute ago", "{} minutes ago")
    } else if seconds < 86_400 {
        (seconds / 3_600, "{} hour ago", "{} hours ago")
    } else {
        (seconds / 86_400, "{} day ago", "{} days ago")
    };
    let key = if amount == 1 { singular } else { plural };
    crate::i18n::replace_for(language, key, &amount.to_string())
}

/// Reads and decodes an embedded project thumbnail without loading its geometry.
pub fn decode_thumbnail(path: &Path) -> Option<Arc<RenderImage>> {
    let bytes = std::fs::read(path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let encoded = json.get("thumbnail")?.as_str()?;
    let png = BASE64.decode(encoded).ok()?;
    let pixels = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    Some(Arc::new(RenderImage::new(smallvec![Frame::new(pixels)])))
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(crate::i18n::t("Design"))
        .to_owned()
}

/// Renders the full-window project gallery.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let now = SystemTime::now();
    let query = app.home_query.trim().to_lowercase();
    let designs: Vec<_> = app
        .home_designs
        .iter()
        .filter(|design| query.is_empty() || stem(&design.path).to_lowercase().contains(&query))
        .cloned()
        .collect();
    let mut grid = div().flex().flex_row().flex_wrap().gap(theme.space(4.0));
    grid = grid.child(new_design_card(app, cx));
    for (index, design) in designs.iter().enumerate() {
        grid = grid.child(design_card(app, design, index, now, cx));
    }

    div()
        .id("project-home")
        .size_full()
        .bg(theme.well)
        .text_color(theme.text)
        .track_focus(&app.home_focus)
        .on_key_down(cx.listener(Free3dApp::home_key_down))
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(72.0))
                .px(theme.space(6.0))
                .flex()
                .items_center()
                .gap(theme.space(4.0))
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.panel)
                .child(
                    div()
                        .text_size(px(24.0))
                        .font_weight(FontWeight::BOLD)
                        .child(crate::i18n::t("Free3D")),
                )
                .child(
                    div()
                        .ml_auto()
                        .w(px(340.0))
                        .h(px(38.0))
                        .px(theme.space(2.5))
                        .flex()
                        .items_center()
                        .gap(theme.space(2.0))
                        .rounded(px(theme.radius_control))
                        .bg(theme.elevated)
                        .border_1()
                        .border_color(theme.border_strong)
                        .child(ui::glyph(theme, "search"))
                        .child(if app.home_query.is_empty() {
                            div()
                                .text_color(theme.text_faint)
                                .child(crate::i18n::t("Search designs…"))
                        } else {
                            div().flex_1().child(app.home_query.clone())
                        })
                        .child(div().w(px(1.5)).h(px(16.0)).bg(theme.accent)),
                )
                .child(
                    div()
                        .id("home-import")
                        .h(px(38.0))
                        .px(theme.space(3.0))
                        .flex()
                        .items_center()
                        .gap(theme.space(1.5))
                        .rounded(px(theme.radius_control))
                        .bg(theme.accent)
                        .text_color(theme.on_accent)
                        .cursor_pointer()
                        .child(ui::glyph(theme, "import").text_color(theme.on_accent))
                        .child(crate::i18n::t("Import"))
                        .on_click(cx.listener(|this, _, _window, cx| this.import_from_home(cx))),
                ),
        )
        .child(
            div()
                .id("home-design-scroll")
                .flex_1()
                .overflow_y_scroll()
                .p(theme.space(6.0))
                .child(
                    div()
                        .mb(theme.space(4.0))
                        .text_size(px(theme.text_lg + 3.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(crate::i18n::t("Designs")),
                )
                .child(grid)
                .when(app.home_designs.is_empty(), |root| {
                    root.child(
                        div()
                            .mt(theme.space(6.0))
                            .text_color(theme.text_muted)
                            .child(crate::i18n::t("Your designs will appear here.")),
                    )
                }),
        )
}

fn new_design_card(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    div()
        .id("new-design-card")
        .w(px(200.0))
        .h(px(160.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(theme.space(2.0))
        .rounded(px(theme.radius_panel))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.panel)
        .hover(|card| card.bg(theme.elevated).border_color(theme.accent))
        .cursor_pointer()
        .child(
            ui::glyph(theme, "add")
                .size(px(36.0))
                .text_color(theme.accent),
        )
        .child(crate::i18n::t("New Design"))
        .on_click(cx.listener(|this, _, _window, cx| this.new_design(cx)))
}

fn design_card(
    app: &Free3dApp,
    design: &DesignEntry,
    index: usize,
    now: SystemTime,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    let path = design.path.clone();
    let card_path = path.clone();
    let menu_path = path.clone();
    let rename_path = path.clone();
    let duplicate_path = path.clone();
    let delete_path = path.clone();
    let reveal_path = path.clone();
    let name = stem(&path);
    let age = now.duration_since(design.modified).unwrap_or_default();
    let preview = app.home_thumbnails.get(&path).and_then(Clone::clone);
    let menu_open = app.home_menu_path.as_ref() == Some(&path);
    let renaming = app.home_rename_path.as_ref() == Some(&path);
    div()
        .id(("design-card", index))
        .group("design-card")
        .relative()
        .w(px(200.0))
        .h(px(160.0))
        .overflow_hidden()
        .rounded(px(theme.radius_panel))
        .border_1()
        .border_color(theme.border)
        .bg(theme.panel)
        .hover(|card| card.border_color(theme.border_strong).bg(theme.elevated))
        .cursor_pointer()
        .child(
            div()
                .h(px(112.0))
                .w_full()
                .bg(theme.elevated)
                .flex()
                .items_center()
                .justify_center()
                .when_some(preview, |area, image| {
                    area.child(img(ImageSource::Render(image)).size_full())
                })
                .when(
                    !app.home_thumbnails.get(&path).is_some_and(Option::is_some),
                    |area| {
                        area.child(
                            ui::glyph(theme, "project")
                                .size(px(38.0))
                                .text_color(theme.text_faint),
                        )
                    },
                ),
        )
        .child(
            div()
                .h(px(48.0))
                .px(theme.space(2.0))
                .flex()
                .flex_col()
                .justify_center()
                .child(if renaming {
                    app.home_rename_buffer.clone()
                } else {
                    name
                })
                .child(
                    div()
                        .text_size(px(theme.text_sm))
                        .text_color(theme.text_muted)
                        .child(relative_time(age, crate::i18n::lang())),
                ),
        )
        .child(
            div()
                .id(("design-menu-button", index))
                .absolute()
                .top(theme.space(2.0))
                .right(theme.space(2.0))
                .invisible()
                .group_hover("design-card", |button| button.visible())
                .size(px(28.0))
                .rounded(px(theme.radius_control))
                .bg(theme.panel)
                .flex()
                .items_center()
                .justify_center()
                .child("•••")
                .on_click(
                    cx.listener(move |this, _, _, cx| this.toggle_home_menu(menu_path.clone(), cx)),
                ),
        )
        .when(menu_open, |card| {
            card.child(
                ui::surface_elevated(theme)
                    .absolute()
                    .top(px(36.0))
                    .right(theme.space(2.0))
                    .w(px(142.0))
                    .p(theme.space(1.0))
                    .flex()
                    .flex_col()
                    .child(menu_row(
                        app,
                        "Rename",
                        rename_path,
                        Free3dApp::begin_home_rename,
                        cx,
                    ))
                    .child(menu_row(
                        app,
                        "Duplicate",
                        duplicate_path,
                        Free3dApp::duplicate_design,
                        cx,
                    ))
                    .child(menu_row(
                        app,
                        "Delete",
                        delete_path,
                        Free3dApp::trash_design,
                        cx,
                    ))
                    .child(menu_row(
                        app,
                        "Reveal in Finder",
                        reveal_path,
                        Free3dApp::reveal_design,
                        cx,
                    )),
            )
        })
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            if event.click_count() >= 1
                && this.home_menu_path.is_none()
                && this.home_rename_path.is_none()
            {
                this.open_recent(card_path.clone(), window, cx);
            }
        }))
}

fn menu_row(
    app: &Free3dApp,
    label: &'static str,
    path: PathBuf,
    action: fn(&mut Free3dApp, PathBuf, &mut gpui::Window, &mut Context<Free3dApp>),
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    div()
        .id(label)
        .px(theme.space(2.0))
        .py(theme.space(1.5))
        .rounded(px(theme.radius_control))
        .text_size(px(theme.text_sm))
        .hover(|row| row.bg(theme.hover))
        .child(crate::i18n::t(label))
        .on_click(cx.listener(move |this, _, window, cx| action(this, path.clone(), window, cx)))
}

/// Produces an encoded PNG thumbnail from a BGRA viewport frame.
pub fn encode_thumbnail(width: u32, height: u32, mut bgra: Vec<u8>) -> Option<String> {
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let rgba = RgbaImage::from_raw(width, height, bgra)?;
    let image = image::DynamicImage::ImageRgba8(rgba);
    let resized = if width.max(height) > 512 {
        image.resize(512, 512, image::imageops::FilterType::Lanczos3)
    } else {
        image
    };
    let mut png = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(BASE64.encode(png.into_inner()))
}

/// Rewrites a recent-file list after a successful rename.
pub fn rename_recent_paths(recent: &mut [PathBuf], old: &Path, new: &Path) {
    for path in recent {
        if path == old {
            *path = new.to_owned();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_are_unique() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("Part.f3d");
        std::fs::write(&source, b"x").unwrap();
        assert_eq!(
            unique_copy_path(&source, Lang::En),
            directory.path().join("Part Copy.f3d")
        );
        std::fs::write(directory.path().join("Part Copy.f3d"), b"x").unwrap();
        assert_eq!(
            unique_copy_path(&source, Lang::En),
            directory.path().join("Part Copy 2.f3d")
        );
    }

    #[test]
    fn relative_time_has_english_and_chinese_buckets() {
        assert_eq!(relative_time(Duration::from_secs(5), Lang::En), "Just now");
        assert_eq!(
            relative_time(Duration::from_secs(120), Lang::En),
            "2 minutes ago"
        );
        assert_eq!(
            relative_time(Duration::from_secs(10_800), Lang::ZhCn),
            "3 小时前"
        );
        assert_eq!(
            relative_time(Duration::from_secs(172_800), Lang::ZhCn),
            "2 天前"
        );
    }

    #[test]
    fn rename_updates_recent_paths() {
        let old = PathBuf::from("old.f3d");
        let new = PathBuf::from("new.f3d");
        let mut recent = vec![PathBuf::from("other.f3d"), old.clone()];
        rename_recent_paths(&mut recent, &old, &new);
        assert_eq!(recent, vec![PathBuf::from("other.f3d"), new]);
    }

    #[test]
    fn listing_unions_deduplicates_and_sorts() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.f3d");
        let second = directory.path().join("second.f3d");
        std::fs::write(&first, b"1").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&second, b"2").unwrap();
        let external = tempfile::NamedTempFile::with_suffix(".f3d").unwrap();
        let listed = list_designs_in(
            directory.path(),
            &[first.clone(), external.path().to_owned()],
        );
        assert_eq!(listed.len(), 3);
        let second_position = listed.iter().position(|item| item.path == second).unwrap();
        let first_position = listed.iter().position(|item| item.path == first).unwrap();
        assert!(second_position < first_position);
        assert_eq!(listed.iter().filter(|item| item.path == first).count(), 1);
    }

    #[test]
    fn listing_uses_designs_directory_override() {
        let directory = tempfile::tempdir().unwrap();
        let design = directory.path().join("override.f3d");
        std::fs::write(&design, b"{}").unwrap();
        // SAFETY: this is the only home test that changes this variable.
        unsafe { std::env::set_var("FREE3D_DESIGNS_DIR", directory.path()) };
        assert_eq!(list_designs(&[])[0].path, design);
        unsafe { std::env::remove_var("FREE3D_DESIGNS_DIR") };
    }
}
