//! GPUI-painted A4 drawing sheet and direct drawing interactions.

use glam::DVec2;
use gpui::{
    Bounds, Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder,
    Pixels, Point, Window, canvas, div, point, prelude::*, px, rgba,
};

use crate::{
    app::{DrawingDrag, DrawingSectionState, DrawingTitleField, DrawingTool, Free3dApp},
    drawing::{
        self, DimensionKind, DrawingDim, Projection, SHEET_HEIGHT_MM, SHEET_WIDTH_MM, ViewKind,
    },
    ui,
};

#[derive(Clone, Copy)]
struct SheetLayout {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    px_per_mm: f32,
}

fn layout(window: &Window) -> SheetLayout {
    let viewport = window.viewport_size();
    let available_width = (f32::from(viewport.width) - 260.0).max(297.0);
    let available_height = (f32::from(viewport.height) - 105.0).max(210.0);
    let px_per_mm =
        (available_width / SHEET_WIDTH_MM as f32).min(available_height / SHEET_HEIGHT_MM as f32);
    let width = SHEET_WIDTH_MM as f32 * px_per_mm;
    let height = SHEET_HEIGHT_MM as f32 * px_per_mm;
    SheetLayout {
        left: 230.0 + (available_width - width) * 0.5,
        top: 75.0 + (available_height - height) * 0.5,
        width,
        height,
        px_per_mm,
    }
}

fn event_sheet(position: Point<Pixels>, layout: SheetLayout) -> Option<DVec2> {
    let point = DVec2::new(
        f64::from((position.x - px(layout.left)) / px(layout.px_per_mm)),
        f64::from((position.y - px(layout.top)) / px(layout.px_per_mm)),
    );
    (point.x >= 0.0 && point.y >= 0.0 && point.x <= SHEET_WIDTH_MM && point.y <= SHEET_HEIGHT_MM)
        .then_some(point)
}

/// Renders the paper, projected geometry, labels and projection chooser.
pub fn render(app: &Free3dApp, window: &Window, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let sheet = layout(window);
    let drawing = app.document.read(cx).drawing.clone();
    let projections = app.drawing_projections(cx);
    let bom_rows = app.drawing_bom_rows().to_vec();
    let paint_drawing = drawing.clone();
    let paint_projections = projections.clone();
    let paint_bom_rows = bom_rows.clone();
    let selected = app.drawing_selected_view;

    let mut paper = div()
        .id("drawing-sheet")
        .absolute()
        .left(px(sheet.left))
        .top(px(sheet.top))
        .w(px(sheet.width))
        .h(px(sheet.height))
        .bg(rgba(0xffffffff))
        .border_1()
        .border_color(rgba(0x9aa1aaff))
        .shadow_lg()
        .cursor_crosshair()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                drawing_mouse_down(this, event, window, cx)
            }),
        )
        .on_mouse_move(
            cx.listener(move |this, event: &MouseMoveEvent, window, cx| {
                drawing_mouse_move(this, event, window, cx)
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                drawing_mouse_up(this, event, window, cx);
            }),
        )
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    paint_sheet(
                        bounds,
                        sheet.px_per_mm,
                        &paint_drawing,
                        &paint_projections,
                        &paint_bom_rows,
                        selected,
                        window,
                    )
                },
            )
            .absolute()
            .size_full(),
        );

    for view in &drawing.sheet().views {
        let Some(projected) = projections.get(&view.id) else {
            continue;
        };
        paper = paper.child(
            div()
                .absolute()
                .left(px((view.at.x as f32) * sheet.px_per_mm - 55.0))
                .top(px(((view.at.y + projected.size.y * view.scale * 0.5 + 4.0)
                    as f32)
                    * sheet.px_per_mm))
                .w(px(110.0))
                .text_center()
                .text_size(px(11.0))
                .text_color(rgba(0x20242aff))
                .child(match &view.kind {
                    ViewKind::Section { label, .. } => {
                        format!("剖视 {label}-{label} {}", drawing::scale_label(view.scale))
                    }
                    ViewKind::Detail { label, .. } => {
                        format!("详图 {label} {}", drawing::scale_label(view.scale))
                    }
                    ViewKind::Standard => format!(
                        "{} {}",
                        view.projection.label(),
                        drawing::scale_label(view.scale)
                    ),
                }),
        );
        match &view.kind {
            ViewKind::Section {
                line_a,
                line_b,
                label,
                ..
            } => {
                for endpoint in [line_a, line_b] {
                    paper = paper.child(
                        div()
                            .absolute()
                            .left(px((endpoint.x as f32 + 2.0) * sheet.px_per_mm))
                            .top(px((endpoint.y as f32 - 5.0) * sheet.px_per_mm))
                            .text_size(px(10.0))
                            .text_color(rgba(0x20242aff))
                            .child(label.clone()),
                    );
                }
            }
            ViewKind::Detail {
                center,
                radius,
                label,
                ..
            } => {
                paper = paper.child(
                    div()
                        .absolute()
                        .left(px((center.x as f32 + *radius as f32) * sheet.px_per_mm))
                        .top(px((center.y as f32 - 5.0) * sheet.px_per_mm))
                        .text_size(px(10.0))
                        .text_color(rgba(0x20242aff))
                        .child(label.clone()),
                );
            }
            ViewKind::Standard => {}
        }
    }
    for dim in &drawing.sheet().dims {
        let midpoint = dim_label_point(dim);
        paper = paper.child(
            div()
                .absolute()
                .left(px(midpoint.x as f32 * sheet.px_per_mm - 35.0))
                .top(px(midpoint.y as f32 * sheet.px_per_mm - 14.0))
                .w(px(70.0))
                .text_center()
                .text_size(px(10.0))
                .text_color(rgba(0x20242aff))
                .child(dim.label()),
        );
    }
    for (table_index, table) in drawing.sheet().bom_tables.iter().enumerate() {
        let mut table_element = div()
            .id(("drawing-bom-table", table_index))
            .absolute()
            .left(px(table.at.x as f32 * sheet.px_per_mm))
            .top(px(table.at.y as f32 * sheet.px_per_mm))
            .w(px(108.0 * sheet.px_per_mm))
            .border_1()
            .border_color(rgba(0x20242aff))
            .bg(rgba(0xffffffff));
        for cells in std::iter::once(vec![
            "序号".to_owned(),
            "名称".to_owned(),
            "材质".to_owned(),
            "体积".to_owned(),
            "数量".to_owned(),
        ])
        .chain(bom_rows.iter().map(|row| {
            vec![
                row.number.to_string(),
                row.name.clone(),
                row.material.clone(),
                row.volume_label.clone(),
                row.quantity.to_string(),
            ]
        })) {
            let mut row = div()
                .flex()
                .h(px(7.0 * sheet.px_per_mm))
                .border_b_1()
                .border_color(rgba(0x20242aff));
            for (cell, width) in cells.into_iter().zip([10.0, 32.0, 22.0, 34.0, 10.0]) {
                row = row.child(
                    div()
                        .w(px(width * sheet.px_per_mm))
                        .border_r_1()
                        .border_color(rgba(0x20242aff))
                        .overflow_hidden()
                        .text_center()
                        .text_size(px(9.0))
                        .child(cell),
                );
            }
            table_element = table_element.child(row);
        }
        paper = paper.child(table_element);
    }
    for (index, balloon) in drawing.sheet().balloons.iter().enumerate() {
        if let Some(number) = drawing::bom_number(&bom_rows, balloon.body_id) {
            paper = paper.child(
                div()
                    .id(("drawing-balloon", index))
                    .absolute()
                    .left(px((balloon.at.x as f32 - 4.0) * sheet.px_per_mm))
                    .top(px((balloon.at.y as f32 - 4.0) * sheet.px_per_mm))
                    .size(px(8.0 * sheet.px_per_mm))
                    .rounded_full()
                    .border_1()
                    .border_color(rgba(0x20242aff))
                    .bg(rgba(0xffffffff))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .child(number.to_string()),
            );
        }
    }
    let title = &drawing.sheet().title;
    for (index, (field, value, x, y, width)) in [
        (
            DrawingTitleField::ProjectName,
            title.project_name.as_str(),
            179.0,
            173.0,
            108.0,
        ),
        (
            DrawingTitleField::DrawingNumber,
            title.drawing_number.as_str(),
            179.0,
            184.0,
            38.0,
        ),
        (
            DrawingTitleField::Scale,
            title.scale.as_str(),
            222.0,
            184.0,
            35.0,
        ),
        (
            DrawingTitleField::Units,
            title.units.as_str(),
            262.0,
            184.0,
            27.0,
        ),
        (
            DrawingTitleField::Date,
            title.date.as_str(),
            179.0,
            195.0,
            38.0,
        ),
        (
            DrawingTitleField::Author,
            title.author.as_str(),
            222.0,
            195.0,
            67.0,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let shown = if app.drawing_title_editor == Some(field) {
            app.rename_buffer.as_str()
        } else {
            value
        };
        paper = paper.child(
            div()
                .id(("title-field", index))
                .absolute()
                .left(px(x as f32 * sheet.px_per_mm))
                .top(px(y as f32 * sheet.px_per_mm))
                .w(px(width as f32 * sheet.px_per_mm))
                .h(px(9.0 * sheet.px_per_mm))
                .px(px(2.0))
                .text_size(px(10.0))
                .overflow_hidden()
                .cursor_text()
                .child(shown.to_owned())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.begin_drawing_title_edit(field, window, cx)
                })),
        );
    }

    let mut tabs = div()
        .absolute()
        .left(px(sheet.left))
        .top(px(sheet.top + sheet.height + 8.0))
        .flex()
        .gap(px(4.0));
    for index in 0..drawing.sheets.len() {
        tabs = tabs.child(
            div()
                .id(("drawing-sheet-tab", index))
                .px(px(14.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(if drawing.active_sheet == index {
                    rgba(0xffffffff)
                } else {
                    rgba(0xd4d8ddff)
                })
                .cursor_pointer()
                .child(format!("页 {}", index + 1))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.document.update(cx, |document, cx| {
                        document.drawing.active_sheet = index;
                        document.drawing_changed();
                        cx.notify();
                    });
                    this.drawing_selected_view = None;
                    this.drawing_cache.clear();
                    cx.notify();
                })),
        );
    }
    tabs = tabs.child(
        div()
            .id("drawing-add-sheet")
            .px(px(14.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgba(0xd4d8ddff))
            .cursor_pointer()
            .child("+")
            .on_click(cx.listener(|this, _, _, cx| {
                this.document.update(cx, |document, cx| {
                    document.drawing.add_sheet();
                    document.drawing_changed();
                    cx.notify();
                });
                this.drawing_selected_view = None;
                this.drawing_cache.clear();
                cx.notify();
            })),
    );

    div()
        .absolute()
        .size_full()
        .bg(rgba(0xe8eaedff))
        .child(paper)
        .child(tabs)
        .when_some(app.drawing_pending_view_at, |root, at| {
            root.child(projection_popover(app, at, sheet, cx))
        })
}

fn projection_popover(
    app: &Free3dApp,
    at: DVec2,
    sheet: SheetLayout,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    let mut row = ui::surface_elevated(theme)
        .absolute()
        .left(px(sheet.left + at.x as f32 * sheet.px_per_mm))
        .top(px(sheet.top + at.y as f32 * sheet.px_per_mm))
        .p(theme.space(1.0))
        .flex()
        .gap(theme.space(1.0));
    for projection in [
        Projection::Front,
        Projection::Top,
        Projection::Right,
        Projection::Iso,
    ] {
        row = row.child(
            div()
                .id(("projection-chip", projection as usize))
                .px(theme.space(1.5))
                .py(theme.space(1.0))
                .rounded(px(theme.radius_control))
                .bg(theme.well)
                .hover(|chip| chip.bg(theme.hover))
                .cursor_pointer()
                .text_size(px(theme.text_sm))
                .child(projection.label())
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if let Some(at) = this.drawing_pending_view_at {
                        this.place_drawing_view(projection, at, cx);
                    }
                })),
        );
    }
    row
}

fn paint_sheet(
    bounds: Bounds<Pixels>,
    px_per_mm: f32,
    drawing: &drawing::Drawing,
    projections: &std::collections::HashMap<u64, drawing::ProjectedView>,
    bom_rows: &[drawing::BomRow],
    selected: Option<u64>,
    window: &mut Window,
) {
    let to_px = |p: DVec2| {
        point(
            bounds.origin.x + px(p.x as f32 * px_per_mm),
            bounds.origin.y + px(p.y as f32 * px_per_mm),
        )
    };
    let mut grid = PathBuilder::stroke(px(0.5));
    for x in (10..297).step_by(10) {
        grid.move_to(to_px(DVec2::new(x as f64, 0.0)));
        grid.line_to(to_px(DVec2::new(x as f64, SHEET_HEIGHT_MM)));
    }
    for y in (10..210).step_by(10) {
        grid.move_to(to_px(DVec2::new(0.0, y as f64)));
        grid.line_to(to_px(DVec2::new(SHEET_WIDTH_MM, y as f64)));
    }
    if let Ok(path) = grid.build() {
        window.paint_path(path, rgba(0xe8ebefff));
    }
    for view in &drawing.sheet().views {
        let Some(projected) = projections.get(&view.id) else {
            continue;
        };
        let color = if selected == Some(view.id) {
            rgba(0x146ef5ff)
        } else {
            rgba(0x181b20ff)
        };
        paint_lines(
            window,
            &projected.visible,
            view,
            projected.center,
            to_px,
            color,
            false,
        );
        if !projected.hatch.is_empty() {
            let hatch = projected
                .hatch
                .iter()
                .map(|segment| segment.to_vec())
                .collect::<Vec<_>>();
            paint_lines(
                window,
                &hatch,
                view,
                projected.center,
                to_px,
                rgba(0x4b5058ff),
                false,
            );
        }
        if view.show_centerlines {
            paint_centerlines(window, projected, view, to_px);
        }
        paint_view_marker(window, view, projected, to_px);
        if view.show_hidden {
            paint_lines(
                window,
                &projected.hidden,
                view,
                projected.center,
                to_px,
                rgba(0x8d949dff),
                true,
            );
        }
    }
    for dim in &drawing.sheet().dims {
        paint_dimension(window, dim, to_px);
    }
    let _ = bom_rows;
    for balloon in &drawing.sheet().balloons {
        let mut path = PathBuilder::stroke(px(1.0));
        path.move_to(to_px(balloon.anchor));
        path.line_to(to_px(balloon.at));
        if let Ok(path) = path.build() {
            window.paint_path(path, rgba(0x20242aff));
        }
    }
    paint_title_block(window, to_px);
}

fn paint_lines(
    window: &mut Window,
    lines: &[Vec<DVec2>],
    view: &drawing::DrawingView,
    center: DVec2,
    to_px: impl Copy + Fn(DVec2) -> Point<Pixels>,
    color: gpui::Rgba,
    dashed: bool,
) {
    let mut builder = PathBuilder::stroke(px(1.1));
    if dashed {
        builder = builder.dash_array(&[px(5.0), px(3.0)]);
    }
    for line in lines {
        for (index, point) in line.iter().enumerate() {
            let sheet_point = view.at + (*point - center) * view.scale;
            if index == 0 {
                builder.move_to(to_px(sheet_point));
            } else {
                builder.line_to(to_px(sheet_point));
            }
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_dimension(
    window: &mut Window,
    dim: &DrawingDim,
    to_px: impl Copy + Fn(DVec2) -> Point<Pixels>,
) {
    if dim.kind == DimensionKind::Angle {
        if let (Some(c), Some(d)) = (dim.c, dim.d) {
            let mut path = PathBuilder::stroke(px(1.0));
            path.move_to(to_px(dim.a));
            path.line_to(to_px(dim.b));
            path.move_to(to_px(c));
            path.line_to(to_px(d));
            if let Some(arc) = drawing::angle_dimension_arc(dim) {
                for (index, point) in arc.iter().enumerate() {
                    if index == 0 {
                        path.move_to(to_px(*point));
                    } else {
                        path.line_to(to_px(*point));
                    }
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, rgba(0x20242aff));
            }
            return;
        }
    }
    let (a, b) = dimension_points(dim);
    let direction = (b - a).normalize_or_zero();
    let normal = DVec2::new(-direction.y, direction.x);
    let mut path = PathBuilder::stroke(px(1.0));
    for (from, to) in [(dim.a, a), (dim.b, b), (a, b)] {
        path.move_to(to_px(from));
        path.line_to(to_px(to));
    }
    for (tip, sign) in [(a, 1.0), (b, -1.0)] {
        path.move_to(to_px(tip));
        path.line_to(to_px(tip + direction * sign * 3.0 + normal * 1.2));
        path.move_to(to_px(tip));
        path.line_to(to_px(tip + direction * sign * 3.0 - normal * 1.2));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, rgba(0x20242aff));
    }
}

fn paint_view_marker(
    window: &mut Window,
    view: &drawing::DrawingView,
    projected: &drawing::ProjectedView,
    to_px: impl Copy + Fn(DVec2) -> Point<Pixels>,
) {
    let mut path = PathBuilder::stroke(px(1.1));
    match &view.kind {
        ViewKind::Section { line_a, line_b, .. } => {
            path.move_to(to_px(*line_a));
            path.line_to(to_px(*line_b));
            let direction = (*line_b - *line_a).normalize_or_zero();
            let normal = DVec2::new(-direction.y, direction.x);
            for p in [*line_a, *line_b] {
                path.move_to(to_px(p - normal * 3.0));
                path.line_to(to_px(p + normal * 3.0));
            }
        }
        ViewKind::Detail { center, radius, .. } => {
            for (circle_center, circle_radius) in [
                (*center, *radius),
                (
                    view.at,
                    projected.size.max_element() * view.scale * 0.5 + 3.0,
                ),
            ] {
                let segments = 48;
                for i in 0..=segments {
                    let angle = i as f64 / segments as f64 * std::f64::consts::TAU;
                    let p = circle_center + DVec2::new(angle.cos(), angle.sin()) * circle_radius;
                    if i == 0 {
                        path.move_to(to_px(p));
                    } else {
                        path.line_to(to_px(p));
                    }
                }
            }
        }
        ViewKind::Standard => return,
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, rgba(0x20242aff));
    }
}

fn drawing_mouse_down(
    app: &mut Free3dApp,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<Free3dApp>,
) {
    let sheet_layout = layout(window);
    let Some(position) = event_sheet(event.position, sheet_layout) else {
        return;
    };
    match app.drawing_tool {
        Some(DrawingTool::View) => {
            app.drawing_pending_view_at = Some(position);
        }
        Some(DrawingTool::Dimension) => {
            if let Some((view_id, snapped, scale)) = snap_endpoint(app, position, sheet_layout, cx)
            {
                if let Some((first_view, first)) = app.drawing_pending_dim.take() {
                    if first_view == view_id && first.distance(snapped) > 1.0e-6 {
                        app.document.update(cx, |document, cx| {
                            document.drawing.checkpoint();
                            document
                                .drawing
                                .sheet_mut()
                                .dims
                                .push(DrawingDim::linear(first, snapped, 8.0, scale));
                            document.drawing_changed();
                            cx.notify();
                        });
                    } else {
                        app.drawing_pending_dim = Some((view_id, snapped));
                    }
                } else {
                    app.drawing_pending_dim = Some((view_id, snapped));
                }
            }
        }
        Some(DrawingTool::Section) => {
            if let Some(mut state) = app.drawing_pending_section {
                if let Some(first) = state.first {
                    let delta = position - first;
                    let second = if delta.x.abs() >= delta.y.abs() {
                        DVec2::new(position.x, first.y)
                    } else {
                        DVec2::new(first.x, position.y)
                    };
                    if first.distance(second) > 1.0 {
                        create_section_view(app, state.parent_id, first, second, cx);
                        app.drawing_pending_section = None;
                    }
                } else {
                    state.first = Some(position);
                    app.drawing_pending_section = Some(state);
                }
            } else if let Some(id) = hit_view(app, position, cx) {
                app.drawing_pending_section = Some(DrawingSectionState {
                    parent_id: id,
                    first: None,
                });
                app.drawing_selected_view = Some(id);
            }
        }
        Some(DrawingTool::Detail) => {
            if let Some(parent_id) = hit_view(app, position, cx) {
                app.drawing_drag = Some(DrawingDrag::Detail {
                    parent_id,
                    center: position,
                });
            }
        }
        Some(DrawingTool::Radius) | Some(DrawingTool::Diameter) => {
            if let Some((center, point, scale)) = hit_circle(app, position, cx) {
                let diameter = app.drawing_tool == Some(DrawingTool::Diameter);
                app.document.update(cx, |document, cx| {
                    document.drawing.checkpoint();
                    document.drawing.sheet_mut().dims.push(DrawingDim {
                        kind: if diameter {
                            DimensionKind::Diameter
                        } else {
                            DimensionKind::Radius
                        },
                        a: center,
                        b: point,
                        offset: 6.0,
                        value_mm: center.distance(point) / scale * if diameter { 2.0 } else { 1.0 },
                        c: Some(center),
                        d: None,
                    });
                    document.drawing_changed();
                    cx.notify();
                });
            }
        }
        Some(DrawingTool::Angle) => {
            if let Some((view_id, a, b)) = hit_line(app, position, cx) {
                if let Some((first_id, first_a, first_b)) = app.drawing_pending_angle.take() {
                    if first_id == view_id
                        && let Some(value) = drawing::angle_degrees(first_a, first_b, a, b)
                    {
                        app.document.update(cx, |document, cx| {
                            document.drawing.checkpoint();
                            document.drawing.sheet_mut().dims.push(DrawingDim {
                                kind: DimensionKind::Angle,
                                a: first_a,
                                b: first_b,
                                offset: 8.0,
                                value_mm: value,
                                c: Some(a),
                                d: Some(b),
                            });
                            document.drawing_changed();
                            cx.notify();
                        });
                    }
                } else {
                    app.drawing_pending_angle = Some((view_id, a, b));
                }
            }
        }
        Some(DrawingTool::Bom) => {
            app.document.update(cx, |document, cx| {
                document.drawing.checkpoint();
                document
                    .drawing
                    .sheet_mut()
                    .bom_tables
                    .push(drawing::BomTable { at: position });
                document.drawing_changed();
                cx.notify();
            });
            app.drawing_tool = None;
        }
        Some(DrawingTool::Balloon) => {
            let resolved = {
                let document = app.document.read(cx);
                document.drawing.sheet().views.iter().find_map(|view| {
                    let projected = app.drawing_cache.get(&view.id)?.1.clone();
                    drawing::resolve_balloon_body(view, &projected, position, 4.0)
                        .map(|body| (view.id, body))
                })
            };
            if let Some((view_id, body_id)) = resolved {
                app.document.update(cx, |document, cx| {
                    document.drawing.checkpoint();
                    document
                        .drawing
                        .sheet_mut()
                        .balloons
                        .push(drawing::Balloon {
                            view_id,
                            body_id,
                            anchor: position,
                            at: position + DVec2::new(14.0, -14.0),
                        });
                    document.drawing_changed();
                    cx.notify();
                });
                app.drawing_tool = None;
            }
        }
        None => {
            let (balloon, bom, dimension, view) = {
                let document = app.document.read(cx);
                let balloon = document
                    .drawing
                    .sheet()
                    .balloons
                    .iter()
                    .position(|balloon| balloon.at.distance(position) < 6.0);
                let bom = document
                    .drawing
                    .sheet()
                    .bom_tables
                    .iter()
                    .position(|table| {
                        let height = 7.0 * (app.drawing_bom_rows().len() + 1) as f64;
                        position.x >= table.at.x
                            && position.x <= table.at.x + 108.0
                            && position.y >= table.at.y
                            && position.y <= table.at.y + height
                    });
                let dimension = document
                    .drawing
                    .sheet()
                    .dims
                    .iter()
                    .position(|dim| dim_label_point(dim).distance(position) < 7.0);
                let view = document
                    .drawing
                    .sheet()
                    .views
                    .iter()
                    .rev()
                    .find(|view| {
                        app.drawing_cache
                            .get(&view.id)
                            .is_some_and(|(_, projected)| {
                                let half = projected.size * view.scale * 0.5 + DVec2::splat(5.0);
                                (position - view.at).abs().cmple(half).all()
                            })
                    })
                    .map(|view| (view.id, position - view.at));
                (balloon, bom, dimension, view)
            };
            if let Some(index) = balloon {
                let at = app.document.read(cx).drawing.sheet().balloons[index].at;
                app.document
                    .update(cx, |document, _| document.drawing.checkpoint());
                app.drawing_drag = Some(DrawingDrag::Balloon {
                    index,
                    grab: position - at,
                });
            } else if let Some(index) = bom {
                let at = app.document.read(cx).drawing.sheet().bom_tables[index].at;
                app.document
                    .update(cx, |document, _| document.drawing.checkpoint());
                app.drawing_drag = Some(DrawingDrag::Bom {
                    index,
                    grab: position - at,
                });
            } else if let Some(index) = dimension {
                app.document
                    .update(cx, |document, _| document.drawing.checkpoint());
                app.drawing_selected_dim = Some(index);
                app.drawing_selected_view = None;
                app.drawing_drag = Some(DrawingDrag::Dimension { index });
            } else if let Some((id, grab)) = view {
                app.drawing_selected_view = Some(id);
                app.drawing_selected_dim = None;
                app.document
                    .update(cx, |document, _| document.drawing.checkpoint());
                app.drawing_drag = Some(DrawingDrag::View { id, grab });
            } else {
                app.drawing_selected_view = None;
                app.drawing_selected_dim = None;
            }
        }
    }
    cx.notify();
}

fn drawing_mouse_move(
    app: &mut Free3dApp,
    event: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut Context<Free3dApp>,
) {
    if event.pressed_button != Some(MouseButton::Left) {
        return;
    }
    let Some(position) = event_sheet(event.position, layout(window)) else {
        return;
    };
    let Some(drag) = app.drawing_drag else { return };
    app.document.update(cx, |document, cx| {
        match drag {
            DrawingDrag::View { id, grab } => {
                if let Some(view) = document
                    .drawing
                    .sheet_mut()
                    .views
                    .iter_mut()
                    .find(|view| view.id == id)
                {
                    view.at = position - grab;
                }
            }
            DrawingDrag::Dimension { index } => {
                if let Some(dim) = document.drawing.sheet_mut().dims.get_mut(index) {
                    let direction = (dim.b - dim.a).normalize_or_zero();
                    dim.offset = (position - dim.a).dot(DVec2::new(-direction.y, direction.x));
                }
            }
            DrawingDrag::Detail { .. } => {}
            DrawingDrag::Bom { index, grab } => {
                if let Some(table) = document.drawing.sheet_mut().bom_tables.get_mut(index) {
                    table.at = position - grab;
                }
            }
            DrawingDrag::Balloon { index, grab } => {
                if let Some(balloon) = document.drawing.sheet_mut().balloons.get_mut(index) {
                    balloon.at = position - grab;
                }
            }
        }
        document.drawing_changed();
        cx.notify();
    });
}

fn drawing_mouse_up(
    app: &mut Free3dApp,
    event: &MouseUpEvent,
    window: &mut Window,
    cx: &mut Context<Free3dApp>,
) {
    let drag = app.drawing_drag.take();
    if let Some(DrawingDrag::Detail { parent_id, center }) = drag
        && let Some(position) = event_sheet(event.position, layout(window))
    {
        let radius = center.distance(position);
        if radius > 2.0 {
            let parent = app
                .document
                .read(cx)
                .drawing
                .sheet()
                .views
                .iter()
                .find(|view| view.id == parent_id)
                .cloned();
            if let Some(parent) = parent {
                let label = detail_label(app, cx);
                let at = parent.at + DVec2::new(95.0, 0.0);
                let detail_scale = app.drawing_detail_scale;
                let id = app.document.update(cx, |document, cx| {
                    let id = document.drawing.add_derived_view(
                        parent.projection,
                        at,
                        parent.scale * detail_scale,
                        ViewKind::Detail {
                            parent_id,
                            center,
                            radius,
                            label,
                        },
                    );
                    document.drawing_changed();
                    cx.notify();
                    id
                });
                app.drawing_selected_view = Some(id);
                app.drawing_cache.remove(&id);
            }
        }
    }
    cx.notify();
}

fn hit_view(app: &Free3dApp, position: DVec2, cx: &Context<Free3dApp>) -> Option<u64> {
    app.document
        .read(cx)
        .drawing
        .sheet()
        .views
        .iter()
        .rev()
        .find(|view| {
            app.drawing_cache
                .get(&view.id)
                .is_some_and(|(_, projected)| {
                    let half = projected.size * view.scale * 0.5 + DVec2::splat(5.0);
                    (position - view.at).abs().cmple(half).all()
                })
        })
        .map(|view| view.id)
}

fn create_section_view(
    app: &mut Free3dApp,
    parent_id: u64,
    line_a: DVec2,
    line_b: DVec2,
    cx: &mut Context<Free3dApp>,
) {
    let parent = app
        .document
        .read(cx)
        .drawing
        .sheet()
        .views
        .iter()
        .find(|v| v.id == parent_id)
        .cloned();
    let Some(parent) = parent else { return };
    let view_dir = parent
        .view_dir
        .unwrap_or(parent.projection.view_dir())
        .normalize();
    let right = if glam::DVec3::Z.cross(view_dir).length_squared() > 1.0e-8 {
        glam::DVec3::Z.cross(view_dir).normalize()
    } else {
        glam::DVec3::X
    };
    let up = view_dir.cross(right).normalize();
    let line = (line_b - line_a).normalize_or_zero();
    let plane_normal = (right * -line.y + up * line.x).normalize_or_zero();
    let mid = (line_a + line_b) * 0.5 - parent.at;
    let projected_center = app
        .drawing_cache
        .get(&parent_id)
        .map_or(DVec2::ZERO, |(_, projected)| projected.center);
    let plane_point = projected_center + mid / parent.scale;
    let plane_origin = right * plane_point.x + up * plane_point.y;
    let label = section_label(app, cx);
    let at = parent.at + DVec2::new(95.0, 0.0);
    let id = app.document.update(cx, |document, cx| {
        let id = document.drawing.add_derived_view(
            parent.projection,
            at,
            parent.scale,
            ViewKind::Section {
                parent_id,
                line_a,
                line_b,
                plane_origin,
                plane_normal,
                label,
            },
        );
        document.drawing_changed();
        cx.notify();
        id
    });
    app.drawing_selected_view = Some(id);
    app.drawing_cache.remove(&id);
}

fn section_label(app: &Free3dApp, cx: &Context<Free3dApp>) -> String {
    let count = app
        .document
        .read(cx)
        .drawing
        .sheet()
        .views
        .iter()
        .filter(|v| matches!(v.kind, ViewKind::Section { .. }))
        .count();
    ((b'A' + count.min(25) as u8) as char).to_string()
}

fn detail_label(app: &Free3dApp, cx: &Context<Free3dApp>) -> String {
    let count = app
        .document
        .read(cx)
        .drawing
        .sheet()
        .views
        .iter()
        .filter(|v| matches!(v.kind, ViewKind::Detail { .. }))
        .count();
    ((b'B' + count.min(24) as u8) as char).to_string()
}

fn hit_circle(
    app: &Free3dApp,
    position: DVec2,
    cx: &Context<Free3dApp>,
) -> Option<(DVec2, DVec2, f64)> {
    for view in app.document.read(cx).drawing.sheet().views.iter().rev() {
        let Some((_, projected)) = app.drawing_cache.get(&view.id) else {
            continue;
        };
        for line in &projected.visible {
            if let Some((center, radius)) = drawing::detect_circle(line) {
                let center = view.at + (center - projected.center) * view.scale;
                let radius = radius * view.scale;
                if (position.distance(center) - radius).abs() < 4.0 {
                    let point = center + (position - center).normalize_or(DVec2::X) * radius;
                    return Some((center, point, view.scale));
                }
            }
        }
    }
    None
}

fn hit_line(
    app: &Free3dApp,
    position: DVec2,
    cx: &Context<Free3dApp>,
) -> Option<(u64, DVec2, DVec2)> {
    let mut best = None;
    for view in &app.document.read(cx).drawing.sheet().views {
        let Some((_, projected)) = app.drawing_cache.get(&view.id) else {
            continue;
        };
        for line in &projected.visible {
            for edge in line.windows(2) {
                let a = view.at + (edge[0] - projected.center) * view.scale;
                let b = view.at + (edge[1] - projected.center) * view.scale;
                let ab = b - a;
                let t = ((position - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let distance = position.distance(a + ab * t);
                if distance < 4.0 && best.as_ref().is_none_or(|(_, _, _, old)| distance < *old) {
                    best = Some((view.id, a, b, distance));
                }
            }
        }
    }
    best.map(|(id, a, b, _)| (id, a, b))
}

fn snap_endpoint(
    app: &Free3dApp,
    position: DVec2,
    sheet: SheetLayout,
    cx: &Context<Free3dApp>,
) -> Option<(u64, DVec2, f64)> {
    let document = app.document.read(cx);
    let mut best: Option<(u64, DVec2, f64, f64)> = None;
    for view in &document.drawing.sheet().views {
        let Some((_, projected)) = app.drawing_cache.get(&view.id) else {
            continue;
        };
        for point in projected
            .visible
            .iter()
            .flat_map(|line| [line.first(), line.last()])
            .flatten()
        {
            let endpoint = view.at + (*point - projected.center) * view.scale;
            let distance = endpoint.distance(position);
            if distance * sheet.px_per_mm as f64 <= 6.0
                && best.as_ref().is_none_or(|best| distance < best.3)
            {
                best = Some((view.id, endpoint, view.scale, distance));
            }
        }
    }
    best.map(|(id, point, scale, _)| (id, point, scale))
}

fn dimension_points(dim: &DrawingDim) -> (DVec2, DVec2) {
    let direction = (dim.b - dim.a).normalize_or_zero();
    let normal = DVec2::new(-direction.y, direction.x);
    (dim.a + normal * dim.offset, dim.b + normal * dim.offset)
}

fn dim_label_point(dim: &DrawingDim) -> DVec2 {
    if dim.kind == DimensionKind::Angle
        && let Some(arc) = drawing::angle_dimension_arc(dim)
    {
        return arc[arc.len() / 2];
    }
    let (a, b) = dimension_points(dim);
    (a + b) * 0.5
}

fn paint_centerlines(
    window: &mut Window,
    projected: &drawing::ProjectedView,
    view: &drawing::DrawingView,
    to_px: impl Copy + Fn(DVec2) -> Point<Pixels>,
) {
    let mut path = PathBuilder::stroke(px(0.8)).dash_array(&[px(6.0), px(2.0), px(1.0), px(2.0)]);
    for line in &projected.visible {
        let Some((center, radius)) = drawing::detect_circle(line) else {
            continue;
        };
        let center = view.at + (center - projected.center) * view.scale;
        let radius = radius * view.scale + 2.0;
        path.move_to(to_px(center - DVec2::X * radius));
        path.line_to(to_px(center + DVec2::X * radius));
        path.move_to(to_px(center - DVec2::Y * radius));
        path.line_to(to_px(center + DVec2::Y * radius));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, rgba(0x555b63ff));
    }
}

fn paint_title_block(window: &mut Window, to_px: impl Copy + Fn(DVec2) -> Point<Pixels>) {
    let mut path = PathBuilder::stroke(px(1.0));
    for (a, b) in [
        (DVec2::new(177.0, 172.0), DVec2::new(292.0, 172.0)),
        (DVec2::new(292.0, 172.0), DVec2::new(292.0, 205.0)),
        (DVec2::new(292.0, 205.0), DVec2::new(177.0, 205.0)),
        (DVec2::new(177.0, 205.0), DVec2::new(177.0, 172.0)),
        (DVec2::new(177.0, 183.0), DVec2::new(292.0, 183.0)),
        (DVec2::new(177.0, 194.0), DVec2::new(292.0, 194.0)),
        (DVec2::new(220.0, 183.0), DVec2::new(220.0, 205.0)),
        (DVec2::new(260.0, 183.0), DVec2::new(260.0, 194.0)),
    ] {
        path.move_to(to_px(a));
        path.line_to(to_px(b));
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, rgba(0x20242aff));
    }
}
