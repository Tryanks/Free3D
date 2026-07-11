//! Floating read-only mass, interference, and validity results.

use gpui::{Context, FontWeight, div, prelude::*, px};

use crate::{
    app::{Free3dApp, InspectionCard},
    ui,
};

/// Renders the current inspection result above the lower-right viewport.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> Option<impl IntoElement> {
    let result = app.inspection_card.as_ref()?;
    let theme = &app.theme;
    let (title, lines, pairs) = match result {
        InspectionCard::Properties(Ok(value)) => {
            let unit = app.units.symbol();
            (
                "属性",
                vec![
                    format!(
                        "体积      {:.6} {unit}³",
                        app.units.display_volume(value.volume)
                    ),
                    format!(
                        "表面积    {:.6} {unit}²",
                        app.units.display_area(value.area)
                    ),
                    format!(
                        "质心 X    {:.6} {unit}",
                        app.units.display_value(value.center.x)
                    ),
                    format!(
                        "质心 Y    {:.6} {unit}",
                        app.units.display_value(value.center.y)
                    ),
                    format!(
                        "质心 Z    {:.6} {unit}",
                        app.units.display_value(value.center.z)
                    ),
                    format!(
                        "I1        {:.6} {unit}⁵",
                        app.units.display_inertia(value.principal_inertia[0])
                    ),
                    format!(
                        "I2        {:.6} {unit}⁵",
                        app.units.display_inertia(value.principal_inertia[1])
                    ),
                    format!(
                        "I3        {:.6} {unit}⁵",
                        app.units.display_inertia(value.principal_inertia[2])
                    ),
                ],
                None,
            )
        }
        InspectionCard::Properties(Err(error)) => ("属性", vec![error.clone()], None),
        InspectionCard::Validity {
            body_name,
            issues: Ok(issues),
        } if issues.is_empty() => ("检查几何", vec![format!("✓ 有效  {body_name}")], None),
        InspectionCard::Validity {
            body_name,
            issues: Ok(issues),
        } => (
            "检查几何",
            std::iter::once(format!("⚠ {body_name}"))
                .chain(issues.iter().cloned())
                .collect(),
            None,
        ),
        InspectionCard::Validity {
            issues: Err(error), ..
        } => ("检查几何", vec![error.clone()], None),
        InspectionCard::Interference(Err(error)) => ("干涉检查", vec![error.clone()], None),
        InspectionCard::Interference(Ok(found)) if found.is_empty() => {
            ("干涉检查", vec!["✓ 无干涉".to_owned()], None)
        }
        InspectionCard::Interference(Ok(found)) => ("干涉检查", Vec::new(), Some(found)),
    };

    let mut body = div().flex().flex_col().gap(theme.space(1.0));
    for line in lines {
        body = body.child(div().text_size(px(theme.text_md)).child(line));
    }
    if let Some(pairs) = pairs {
        for (index, pair) in pairs.iter().enumerate() {
            let label = format!(
                "{} ↔ {}    {:.6} {}³",
                pair.first_name,
                pair.second_name,
                app.units.display_volume(pair.volume),
                app.units.symbol()
            );
            body = body.child(
                div()
                    .id(("interference-pair", index))
                    .px(theme.space(1.0))
                    .py(theme.space(1.0))
                    .rounded(px(theme.radius_control))
                    .hover(|row| row.bg(theme.hover))
                    .cursor_pointer()
                    .child(label)
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_interference(index, cx)),
                    ),
            );
        }
    }
    Some(
        div().absolute().right(px(24.0)).top(px(116.0)).child(
            ui::surface_elevated(theme)
                .id("inspection-card")
                .w(px(340.0))
                .p(theme.space(2.0))
                .flex()
                .flex_col()
                .gap(theme.space(2.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_inspection(cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_color(theme.text_faint).child("Esc")),
                )
                .child(body),
        ),
    )
}
