mod adaptive;
mod app;
mod assembly;
mod autosave;
mod camera;
mod commands;
pub mod constraint;
mod document;
mod drawing;
mod gizmo;
mod history;
mod home;
mod i18n;
mod inspection;
mod io_formats;
mod kernel;
mod nav;
mod pick;
mod saved_views;
mod settings;
mod sketch;
mod theme;
mod tools;
mod ui;
mod units;
mod viewport;

use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_platform::application;

use app::DuctileApp;
use ui::icons::Assets;

fn main() {
    settings::migrate_legacy_config();
    i18n::init(settings::load().language);
    if std::env::var_os("DUCTILE_IO_CHECK").is_some() {
        let path = std::env::temp_dir().join("ductile-io-check.step");
        let mut document = app::startup_document();
        if document.bodies.is_empty() {
            document.add_primitive(history::PrimitiveKind::Box {
                min: glam::DVec3::ZERO,
                max: glam::DVec3::splat(10.0),
            });
        }
        if let Err(error) = document
            .export(&path)
            .and_then(|()| document.import_step(&path).map(|_| ()))
        {
            eprintln!("IO_CHECK failed: {error}");
            std::process::exit(1);
        }
        let bodies = document.bodies.len();
        let split_path = std::env::temp_dir().join("ductile-io-check-split.step");
        let mut source = document::Document::new();
        source.add_primitive(history::PrimitiveKind::Box {
            min: glam::DVec3::ZERO,
            max: glam::DVec3::splat(10.0),
        });
        source.add_primitive(history::PrimitiveKind::Box {
            min: glam::DVec3::splat(20.0),
            max: glam::DVec3::splat(30.0),
        });
        let mut split = document::Document::new();
        if let Err(error) = source
            .export(&split_path)
            .and_then(|()| split.import_step(&split_path).map(|_| ()))
        {
            eprintln!("IO_CHECK split failed: {error}");
            std::process::exit(1);
        }
        if split.bodies.len() != 2 {
            eprintln!(
                "IO_CHECK split failed: expected 2 bodies, got {}",
                split.bodies.len()
            );
            std::process::exit(1);
        }
        for extension in ["obj", "3mf", "gltf", "iges"] {
            let output = std::env::temp_dir().join(format!("ductile-io-check.{extension}"));
            let result = source
                .export(&output)
                .and_then(|()| std::fs::read(&output).map_err(|error| error.to_string()))
                .and_then(|bytes| {
                    let marker = match extension {
                        "obj" => bytes.windows(2).any(|window| window == b"v "),
                        "3mf" => bytes.starts_with(b"PK"),
                        "gltf" => bytes.windows(9).any(|window| window == b"\"version\""),
                        "iges" => !bytes.is_empty(),
                        _ => false,
                    };
                    marker
                        .then_some(())
                        .ok_or_else(|| format!("missing {extension} marker"))
                });
            if let Err(error) = result {
                eprintln!("IO_CHECK {extension} failed: {error}");
                std::process::exit(1);
            }
            println!("IO_CHECK {extension}=ok");
        }
        let dxf_path = std::env::temp_dir().join("ductile-io-check.dxf");
        let sketch = source.add_sketch(sketch::SketchPlane::xy());
        source.add_sketch_entities(
            sketch,
            [sketch::SketchEntity::Line {
                a: glam::DVec2::ZERO,
                b: glam::DVec2::ONE,
            }],
        );
        if source
            .export(&dxf_path)
            .and_then(|()| std::fs::read_to_string(&dxf_path).map_err(|error| error.to_string()))
            .is_err()
        {
            eprintln!("IO_CHECK dxf failed");
            std::process::exit(1);
        }
        println!("IO_CHECK dxf=ok");
        let ductile_path = std::env::temp_dir().join("ductile-io-check.ductile");
        let expected_bodies = source.bodies.len();
        let expected_history = source.history.len();
        let loaded = source
            .save_to(&ductile_path)
            .and_then(|()| document::Document::load_from(&ductile_path));
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                eprintln!("IO_CHECK ductile failed: {error}");
                std::process::exit(1);
            }
        };
        if loaded.bodies.len() != expected_bodies || loaded.history.len() != expected_history {
            eprintln!(
                "IO_CHECK ductile failed: expected {expected_bodies} bodies/{expected_history} history, got {}/{}",
                loaded.bodies.len(),
                loaded.history.len()
            );
            std::process::exit(1);
        }
        println!("IO_CHECK bodies={bodies} split={} ok", split.bodies.len());
        println!("IO_CHECK ductile=ok");
        std::process::exit(0);
    }
    application().with_assets(Assets).run(|cx: &mut App| {
        // Fit within the primary display (VMs and small screens included).
        let display = cx.primary_display();
        let available = display
            .as_ref()
            .map(|display| display.bounds().size)
            .unwrap_or_else(|| size(px(1440.0), px(900.0)));
        let bounds = Bounds::centered(
            None,
            size(
                px(f32::from(available.width).min(1440.0) - 24.0),
                px(f32::from(available.height).min(900.0) - 48.0),
            ),
            cx,
        );
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let root = cx.new(DuctileApp::new);
                root.update(cx, |app, cx| {
                    app.start_autosave_timer(cx);
                    app.prompt_for_recovery(window, cx);
                });
                let weak = root.downgrade();
                window.on_window_should_close(cx, move |window, cx| {
                    let dirty = weak.upgrade().is_some_and(|root| {
                        let app = root.read(cx);
                        app.document.read(cx).revision != app.saved_revision
                    });
                    if !dirty {
                        return true;
                    }
                    let response = window.prompt(
                        gpui::PromptLevel::Warning,
                        crate::i18n::t("Unsaved changes will be lost. Continue?"),
                        None,
                        &[crate::i18n::t("Continue"), crate::i18n::t("Cancel")],
                        cx,
                    );
                    let weak = weak.clone();
                    window
                        .spawn(cx, async move |cx| {
                            if matches!(response.await, Ok(0)) {
                                let _ = cx.update(|window, cx| {
                                    if let Some(root) = weak.upgrade() {
                                        root.update(cx, |app, cx| app.autosave_now(cx));
                                    }
                                    window.remove_window();
                                });
                            }
                        })
                        .detach();
                    false
                });
                root
            },
        )
        .expect("failed to open the Ductile window");
        cx.activate(true);
    });
}
