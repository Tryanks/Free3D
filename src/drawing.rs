//! Persisted multi-sheet drawing data, OCCT HLR projection and vector export.

use std::{collections::HashMap, fmt::Write as _, path::Path};

use glam::{DVec2, DVec3};
use occt::Shape;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    document::{Body, BodyId, Material},
    units::Units,
};

/// Fixed sheet width, A4 landscape, in millimetres.
pub const SHEET_WIDTH_MM: f64 = 297.0;
/// Fixed sheet height, A4 landscape, in millimetres.
pub const SHEET_HEIGHT_MM: f64 = 210.0;
/// Supported printed view scales, largest first.
pub const SCALE_STEPS: [f64; 4] = [1.0, 0.5, 0.2, 0.1];

/// Geometry source for a projected view.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum DrawingSource {
    /// Every visible body in the document.
    #[default]
    AllBodies,
}

/// Standard orthographic drawing projections.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// Looking along negative Y, with Z up.
    #[default]
    Front,
    /// Looking down negative Z.
    Top,
    /// Looking along negative X.
    Right,
    /// Equal-axis pictorial projection.
    Iso,
}

impl Projection {
    /// Localized sheet label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Front => crate::i18n::t("Front View"),
            Self::Top => crate::i18n::t("Plan View"),
            Self::Right => crate::i18n::t("Right View"),
            Self::Iso => crate::i18n::t("Isometric"),
        }
    }

    /// Direction from the model toward the viewer, used by OCCT HLR.
    pub fn view_dir(self) -> DVec3 {
        match self {
            Self::Front => -DVec3::Y,
            Self::Top => -DVec3::Z,
            Self::Right => -DVec3::X,
            Self::Iso => DVec3::new(-1.0, -1.0, -1.0).normalize(),
        }
    }
}

/// Extra construction used to derive a drawing view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ViewKind {
    /// An ordinary orthographic or isometric view.
    Standard,
    /// A half-space section derived from a parent view.
    Section {
        /// Parent view identifier.
        parent_id: u64,
        /// Cutting line endpoints in sheet coordinates.
        line_a: DVec2,
        line_b: DVec2,
        /// Cutting plane in model coordinates.
        plane_origin: DVec3,
        plane_normal: DVec3,
        /// Alphabetic section designator.
        label: String,
    },
    /// A circular two-times magnified region of a parent view.
    Detail {
        /// Parent view identifier.
        parent_id: u64,
        /// Detail centre in sheet coordinates.
        center: DVec2,
        /// Marker radius in sheet millimetres.
        radius: f64,
        /// Alphabetic detail designator.
        label: String,
    },
}

impl Default for ViewKind {
    fn default() -> Self {
        Self::Standard
    }
}

/// One placed drawing view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingView {
    /// Stable drawing-local identifier.
    pub id: u64,
    /// Bodies projected by the view.
    pub source: DrawingSource,
    /// Projection orientation.
    pub projection: Projection,
    /// Optional arbitrary direction for derived views.
    #[serde(default)]
    pub view_dir: Option<DVec3>,
    /// View origin on the sheet, in millimetres from its top-left.
    pub at: DVec2,
    /// Printed/model scale (0.5 means 1:2).
    pub scale: f64,
    /// Whether hidden edges are emitted.
    pub show_hidden: bool,
    /// Whether automatic centre marks are emitted.
    #[serde(default = "yes")]
    pub show_centerlines: bool,
    /// Derivation metadata.
    #[serde(default)]
    pub kind: ViewKind,
}

fn yes() -> bool {
    true
}

/// Supported drafting dimensions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum DimensionKind {
    /// Endpoint-to-endpoint distance.
    #[default]
    Linear,
    /// Radius of a fitted circular polyline.
    Radius,
    /// Diameter of a fitted circular polyline.
    Diameter,
    /// Smaller included angle between two lines.
    Angle,
}

/// One sheet-space dimension.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrawingDim {
    /// Dimension kind; absent in W10 files means linear.
    #[serde(default)]
    pub kind: DimensionKind,
    /// First snapped sheet point or first angle-line start.
    pub a: DVec2,
    /// Second snapped sheet point or first angle-line end.
    pub b: DVec2,
    /// Signed perpendicular label offset in sheet millimetres.
    pub offset: f64,
    /// Measured model-space value in millimetres or degrees.
    pub value_mm: f64,
    /// Optional circle centre or second angle-line start.
    #[serde(default)]
    pub c: Option<DVec2>,
    /// Optional point on circle or second angle-line end.
    #[serde(default)]
    pub d: Option<DVec2>,
}

impl DrawingDim {
    /// Creates a conventional linear dimension.
    pub fn linear(a: DVec2, b: DVec2, offset: f64, scale: f64) -> Self {
        Self {
            kind: DimensionKind::Linear,
            a,
            b,
            offset,
            value_mm: dimension_value_mm(a, b, scale),
            c: None,
            d: None,
        }
    }

    /// Drafting text for this dimension.
    pub fn label(&self) -> String {
        match self.kind {
            DimensionKind::Linear => format!("{:.2} mm", self.value_mm),
            DimensionKind::Radius => format!("R{:.2}", self.value_mm),
            DimensionKind::Diameter => format!("⌀{:.2}", self.value_mm),
            DimensionKind::Angle => format!("{:.1}°", self.value_mm),
        }
    }
}

/// Placement of one live bill-of-materials table on a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BomTable {
    /// Top-left table corner in sheet millimetres.
    pub at: DVec2,
}

/// A view-owned item balloon whose number is derived from the live BOM.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Balloon {
    pub view_id: u64,
    pub body_id: BodyId,
    pub anchor: DVec2,
    pub at: DVec2,
}

/// One derived BOM row shared by the sheet and vector exporters.
#[derive(Clone, Debug, PartialEq)]
pub struct BomRow {
    pub number: usize,
    pub name: String,
    pub material: String,
    pub volume: f64,
    pub volume_label: String,
    pub quantity: usize,
    pub bodies: Vec<BodyId>,
}

/// Editable title-block fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TitleBlock {
    /// Project name.
    pub project_name: String,
    /// Drawing number.
    pub drawing_number: String,
    /// Overall scale note.
    pub scale: String,
    /// Creation or issue date.
    pub date: String,
    /// Drawing units.
    pub units: String,
    /// Author name.
    pub author: String,
}

impl Default for TitleBlock {
    fn default() -> Self {
        Self {
            project_name: crate::i18n::t("Untitled Project").into(),
            drawing_number: "DUCTILE-001".into(),
            scale: crate::i18n::t("By View").into(),
            date: current_date(),
            units: "mm".into(),
            author: String::new(),
        }
    }
}

/// One A4 landscape sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// Placed projected views.
    #[serde(default)]
    pub views: Vec<DrawingView>,
    /// Sheet dimensions.
    #[serde(default)]
    pub dims: Vec<DrawingDim>,
    /// Live BOM table placements.
    #[serde(default)]
    pub bom_tables: Vec<BomTable>,
    /// Item balloons attached to projected body geometry.
    #[serde(default)]
    pub balloons: Vec<Balloon>,
    /// Standard bottom-right title block.
    #[serde(default)]
    pub title: TitleBlock,
}

#[derive(Clone, Debug)]
struct DrawingState {
    sheets: Vec<Sheet>,
    active_sheet: usize,
    next_view_id: u64,
}

/// A persistent multi-sheet drawing with lightweight drawing-only history.
#[derive(Clone, Debug, Serialize)]
pub struct Drawing {
    /// Ordered sheets.
    pub sheets: Vec<Sheet>,
    /// Selected sheet tab.
    pub active_sheet: usize,
    /// Next stable view identifier.
    pub next_view_id: u64,
    #[serde(skip)]
    undo: Vec<DrawingState>,
    #[serde(skip)]
    redo: Vec<DrawingState>,
}

impl PartialEq for Drawing {
    fn eq(&self, other: &Self) -> bool {
        self.sheets == other.sheets
            && self.active_sheet == other.active_sheet
            && self.next_view_id == other.next_view_id
    }
}

impl Default for Drawing {
    fn default() -> Self {
        Self {
            sheets: vec![Sheet::default()],
            active_sheet: 0,
            next_view_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct DrawingWire {
    #[serde(default)]
    sheets: Vec<Sheet>,
    #[serde(default)]
    active_sheet: usize,
    #[serde(default = "one")]
    next_view_id: u64,
    #[serde(default)]
    views: Vec<DrawingView>,
    #[serde(default)]
    dims: Vec<DrawingDim>,
}

fn one() -> u64 {
    1
}

impl<'de> Deserialize<'de> for Drawing {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = DrawingWire::deserialize(deserializer)?;
        let sheets = if wire.sheets.is_empty() {
            vec![Sheet {
                views: wire.views,
                dims: wire.dims,
                bom_tables: Vec::new(),
                balloons: Vec::new(),
                title: TitleBlock::default(),
            }]
        } else {
            wire.sheets
        };
        Ok(Self {
            active_sheet: wire.active_sheet.min(sheets.len().saturating_sub(1)),
            sheets,
            next_view_id: wire.next_view_id.max(1),
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }
}

impl Drawing {
    /// Currently selected sheet.
    pub fn sheet(&self) -> &Sheet {
        &self.sheets[self.active_sheet.min(self.sheets.len() - 1)]
    }

    /// Currently selected sheet, mutably.
    pub fn sheet_mut(&mut self) -> &mut Sheet {
        let index = self.active_sheet.min(self.sheets.len() - 1);
        &mut self.sheets[index]
    }

    /// Records the current drawing before an edit.
    pub fn checkpoint(&mut self) {
        if self.undo.len() == 32 {
            self.undo.remove(0);
        }
        self.undo.push(self.state());
        self.redo.clear();
    }

    /// Restores the previous drawing edit.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.state());
        self.restore(previous);
        true
    }

    /// Reapplies one reverted drawing edit.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.state());
        self.restore(next);
        true
    }

    fn state(&self) -> DrawingState {
        DrawingState {
            sheets: self.sheets.clone(),
            active_sheet: self.active_sheet,
            next_view_id: self.next_view_id,
        }
    }

    fn restore(&mut self, state: DrawingState) {
        self.sheets = state.sheets;
        self.active_sheet = state.active_sheet;
        self.next_view_id = state.next_view_id;
    }

    /// Adds a new sheet and selects it.
    pub fn add_sheet(&mut self) {
        self.checkpoint();
        self.sheets.push(Sheet::default());
        self.active_sheet = self.sheets.len() - 1;
    }

    /// Adds a standard view and returns its stable identifier.
    pub fn add_view(&mut self, projection: Projection, at: DVec2, scale: f64) -> u64 {
        self.add_derived_view(projection, at, scale, ViewKind::Standard)
    }

    /// Adds a standard or derived view.
    pub fn add_derived_view(
        &mut self,
        projection: Projection,
        at: DVec2,
        scale: f64,
        kind: ViewKind,
    ) -> u64 {
        self.checkpoint();
        let id = self.next_view_id.max(1);
        self.next_view_id = id + 1;
        let view_dir = match &kind {
            ViewKind::Section { plane_normal, .. } => Some(*plane_normal),
            _ => None,
        };
        self.sheet_mut().views.push(DrawingView {
            id,
            source: DrawingSource::AllBodies,
            projection,
            view_dir,
            at,
            scale,
            show_hidden: false,
            show_centerlines: true,
            kind,
        });
        id
    }
}

fn current_date() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| (duration.as_secs() / 86_400) as i64);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Sampled 2D HLR result in model millimetres, centred around its bounds.
#[derive(Clone, Debug, Default)]
pub struct ProjectedView {
    /// Visible projected curves.
    pub visible: Vec<Vec<DVec2>>,
    /// Source body for every visible polyline, at the matching index.
    pub visible_sources: Vec<Option<BodyId>>,
    /// Occluded projected curves.
    pub hidden: Vec<Vec<DVec2>>,
    /// Source body for every hidden polyline, at the matching index.
    pub hidden_sources: Vec<Option<BodyId>>,
    /// Section-outline curves.
    pub section_outline: Vec<Vec<DVec2>>,
    /// Clipped 45-degree hatch segments.
    pub hatch: Vec<[DVec2; 2]>,
    /// Centre of the unscaled projection bounds.
    pub center: DVec2,
    /// Unscaled projection size.
    pub size: DVec2,
}

/// Runs OCCT HLR for one shape and converts its projector-plane output to 2D.
pub fn shape_hlr(shape: &Shape, projection: Projection) -> Result<ProjectedView, String> {
    shape_hlr_dir(shape, projection.view_dir())
}

/// Runs OCCT HLR along an arbitrary direction.
pub fn shape_hlr_dir(shape: &Shape, view_dir: DVec3) -> Result<ProjectedView, String> {
    let (visible, hidden) = shape.hlr(view_dir, 0.05).map_err(|e| e.to_string())?;
    Ok(projected_view(
        visible.into_iter().map(to_2d).collect(),
        hidden.into_iter().map(to_2d).collect(),
    ))
}

/// Clips a shape by a plane, runs HLR, and generates section hatching.
pub fn shape_section_hlr(
    shape: &Shape,
    plane_origin: DVec3,
    plane_normal: DVec3,
    view_dir: DVec3,
) -> Result<ProjectedView, String> {
    let (visible, hidden, outline) = shape
        .section_hlr(plane_origin, plane_normal, view_dir, 0.05)
        .map_err(|e| e.to_string())?;
    let mut result = projected_view(
        visible.into_iter().map(to_2d).collect(),
        hidden.into_iter().map(to_2d).collect(),
    );
    result.section_outline = outline.into_iter().map(to_2d).collect();
    result.hatch = hatch_outline(&result.section_outline, 3.0);
    Ok(result)
}

fn to_2d(points: Vec<DVec3>) -> Vec<DVec2> {
    points.into_iter().map(|p| DVec2::new(p.x, p.y)).collect()
}

/// Combines projected bodies and computes common projection bounds.
pub fn projected_view(visible: Vec<Vec<DVec2>>, hidden: Vec<Vec<DVec2>>) -> ProjectedView {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for point in visible.iter().chain(&hidden).flatten() {
        min = min.min(*point);
        max = max.max(*point);
    }
    if !min.is_finite() {
        min = DVec2::ZERO;
        max = DVec2::ZERO;
    }
    ProjectedView {
        visible,
        visible_sources: Vec::new(),
        hidden,
        hidden_sources: Vec::new(),
        section_outline: Vec::new(),
        hatch: Vec::new(),
        center: (min + max) * 0.5,
        size: max - min,
    }
}

/// Recomputes live BOM rows, grouping volume and area matches within 0.1%.
pub fn bom_rows(bodies: &[Body], units: Units) -> Vec<BomRow> {
    let mut rows: Vec<(BomRow, f64, f64)> = Vec::new();
    for body in bodies {
        let Ok(properties) = body.shape.volume_properties() else {
            continue;
        };
        let near = |left: f64, right: f64| {
            (left - right).abs() / left.abs().max(right.abs()).max(1.0e-12) <= 0.001
        };
        if let Some((row, _, _)) = rows.iter_mut().find(|(_, volume, area)| {
            near(*volume, properties.volume) && near(*area, properties.area)
        }) {
            row.quantity += 1;
            row.bodies.push(body.id);
            continue;
        }
        let volume = units.display_volume(properties.volume);
        rows.push((
            BomRow {
                number: rows.len() + 1,
                name: body.name.clone(),
                material: material_label(body.material).to_owned(),
                volume,
                volume_label: format!("{volume:.3} {}", volume_unit(units)),
                quantity: 1,
                bodies: vec![body.id],
            },
            properties.volume,
            properties.area,
        ));
    }
    rows.into_iter().map(|(row, _, _)| row).collect()
}

fn material_label(material: Material) -> &'static str {
    if material == Material::default() {
        crate::i18n::t("Default")
    } else if material.metallic >= 0.5 {
        crate::i18n::t("Metal")
    } else if material.roughness >= 0.75 {
        crate::i18n::t("Rubber")
    } else if material.roughness <= 0.1 {
        crate::i18n::t("Glass")
    } else {
        crate::i18n::t("Plastic")
    }
}

fn volume_unit(units: Units) -> &'static str {
    match units {
        Units::Millimeter => "mm³",
        Units::Centimeter => "cm³",
        Units::Meter => "m³",
        Units::Inch => "in³",
    }
}

/// Returns the BOM number containing a stable body identifier.
pub fn bom_number(rows: &[BomRow], body: BodyId) -> Option<usize> {
    rows.iter()
        .find(|row| row.bodies.contains(&body))
        .map(|row| row.number)
}

/// Resolves the closest tagged visible polyline to a sheet-space click.
pub fn resolve_balloon_body(
    view: &DrawingView,
    projected: &ProjectedView,
    click: DVec2,
    tolerance: f64,
) -> Option<BodyId> {
    projected
        .visible
        .iter()
        .zip(&projected.visible_sources)
        .filter_map(|(line, source)| {
            let body = (*source)?;
            let distance = line
                .windows(2)
                .map(|edge| {
                    let a = view.at + (edge[0] - projected.center) * view.scale;
                    let b = view.at + (edge[1] - projected.center) * view.scale;
                    point_segment_distance(click, a, b)
                })
                .fold(f64::INFINITY, f64::min);
            Some((distance, body))
        })
        .filter(|(distance, _)| *distance <= tolerance)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, body)| body)
}

fn point_segment_distance(point: DVec2, a: DVec2, b: DVec2) -> f64 {
    let edge = b - a;
    if edge.length_squared() <= f64::EPSILON {
        return point.distance(a);
    }
    let t = ((point - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
    point.distance(a + edge * t)
}

/// Clips projected polylines to a circular detail region in model coordinates.
pub fn clip_projected_circle(
    mut projected: ProjectedView,
    center: DVec2,
    radius: f64,
) -> ProjectedView {
    let (visible, visible_sources) = clip_tagged_lines_circle(
        &projected.visible,
        &projected.visible_sources,
        center,
        radius,
    );
    let (hidden, hidden_sources) =
        clip_tagged_lines_circle(&projected.hidden, &projected.hidden_sources, center, radius);
    projected.visible = visible;
    projected.hidden = hidden;
    let mut clipped = projected_view(projected.visible, projected.hidden);
    clipped.visible_sources = visible_sources;
    clipped.hidden_sources = hidden_sources;
    clipped.section_outline = projected.section_outline;
    clipped.hatch = projected.hatch;
    clipped
}

fn clip_tagged_lines_circle(
    lines: &[Vec<DVec2>],
    sources: &[Option<BodyId>],
    center: DVec2,
    radius: f64,
) -> (Vec<Vec<DVec2>>, Vec<Option<BodyId>>) {
    let mut result = Vec::new();
    let mut result_sources = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        for edge in line.windows(2) {
            if let Some((a, b)) = clip_segment_circle(edge[0], edge[1], center, radius) {
                result.push(vec![a, b]);
                result_sources.push(sources.get(index).copied().flatten());
            }
        }
    }
    (result, result_sources)
}

fn clip_segment_circle(a: DVec2, b: DVec2, center: DVec2, radius: f64) -> Option<(DVec2, DVec2)> {
    let direction = b - a;
    let relative = a - center;
    let qa = direction.length_squared();
    if qa <= f64::EPSILON {
        return None;
    }
    let qb = 2.0 * relative.dot(direction);
    let qc = relative.length_squared() - radius * radius;
    let discriminant = qb * qb - 4.0 * qa * qc;
    let mut parameters = vec![0.0, 1.0];
    if discriminant >= 0.0 {
        let root = discriminant.sqrt();
        parameters.push((-qb - root) / (2.0 * qa));
        parameters.push((-qb + root) / (2.0 * qa));
    }
    parameters.sort_by(f64::total_cmp);
    parameters.dedup_by(|left, right| (*left - *right).abs() < 1.0e-9);
    for pair in parameters.windows(2) {
        let start = pair[0].clamp(0.0, 1.0);
        let end = pair[1].clamp(0.0, 1.0);
        if end - start > 1.0e-9 {
            let middle = a + direction * ((start + end) * 0.5);
            if middle.distance_squared(center) <= radius * radius + 1.0e-8 {
                return Some((a + direction * start, a + direction * end));
            }
        }
    }
    None
}

/// Generates 45-degree hatch segments by even/odd clipping against outline edges.
pub fn hatch_outline(outline: &[Vec<DVec2>], spacing: f64) -> Vec<[DVec2; 2]> {
    let points = outline.iter().flatten().copied().collect::<Vec<_>>();
    if points.is_empty() || spacing <= 0.0 {
        return Vec::new();
    }
    let min_c = points
        .iter()
        .map(|p| p.x - p.y)
        .fold(f64::INFINITY, f64::min);
    let max_c = points
        .iter()
        .map(|p| p.x - p.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut result = Vec::new();
    let mut c = (min_c / spacing).floor() * spacing;
    while c <= max_c + spacing * 0.5 {
        let mut hits = Vec::new();
        for line in outline {
            for edge in line.windows(2) {
                let ca = edge[0].x - edge[0].y - c;
                let cb = edge[1].x - edge[1].y - c;
                if (ca <= 0.0 && cb > 0.0) || (cb <= 0.0 && ca > 0.0) {
                    let t = ca / (ca - cb);
                    let p = edge[0].lerp(edge[1], t);
                    hits.push((p.x + p.y, p));
                }
            }
        }
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        for pair in hits.chunks_exact(2) {
            if pair[0].1.distance_squared(pair[1].1) > 1.0e-8 {
                result.push([pair[0].1, pair[1].1]);
            }
        }
        c += spacing;
    }
    result
}

/// Fits a sampled circular polyline, returning centre and radius on success.
pub fn detect_circle(line: &[DVec2]) -> Option<(DVec2, f64)> {
    if line.len() < 8 {
        return None;
    }
    let p1 = line[0];
    let p2 = line[line.len() / 3];
    let p3 = line[line.len() * 2 / 3];
    let d = 2.0 * (p1.x * (p2.y - p3.y) + p2.x * (p3.y - p1.y) + p3.x * (p1.y - p2.y));
    if d.abs() < 1.0e-8 {
        return None;
    }
    let q1 = p1.length_squared();
    let q2 = p2.length_squared();
    let q3 = p3.length_squared();
    let center = DVec2::new(
        (q1 * (p2.y - p3.y) + q2 * (p3.y - p1.y) + q3 * (p1.y - p2.y)) / d,
        (q1 * (p3.x - p2.x) + q2 * (p1.x - p3.x) + q3 * (p2.x - p1.x)) / d,
    );
    let radius = center.distance(p1);
    let tolerance = radius.max(1.0) * 0.015;
    (radius.is_finite()
        && radius > 1.0e-6
        && line
            .iter()
            .all(|p| (p.distance(center) - radius).abs() <= tolerance))
    .then_some((center, radius))
}

/// Returns the smaller included angle in degrees for two line segments.
pub fn angle_degrees(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> Option<f64> {
    let u = (b - a).try_normalize()?;
    let v = (d - c).try_normalize()?;
    let angle = u.dot(v).abs().clamp(-1.0, 1.0).acos().to_degrees();
    (angle > 1.0e-6 && angle < 180.0 - 1.0e-6).then_some(angle)
}

/// Samples the smaller angle-dimension arc between a dimension's two lines.
pub fn angle_dimension_arc(dim: &DrawingDim) -> Option<Vec<DVec2>> {
    let (c, d) = (dim.c?, dim.d?);
    let u = dim.b - dim.a;
    let v = d - c;
    let cross = u.perp_dot(v);
    if cross.abs() < 1.0e-9 {
        return None;
    }
    let delta = c - dim.a;
    let center = dim.a + u * (delta.perp_dot(v) / cross);
    let start = u.y.atan2(u.x);
    let end = v.y.atan2(v.x);
    let mut sweep = (end - start).rem_euclid(std::f64::consts::TAU);
    if sweep > std::f64::consts::PI {
        sweep -= std::f64::consts::TAU;
    }
    let radius = dim.offset.abs().max(6.0);
    Some(
        (0..=24)
            .map(|index| {
                let angle = start + sweep * index as f64 / 24.0;
                center + DVec2::new(angle.cos(), angle.sin()) * radius
            })
            .collect(),
    )
}

/// Chooses the largest standard scale fitting within the supplied box.
pub fn auto_fit_scale(model_size: DVec2, available_mm: DVec2) -> f64 {
    SCALE_STEPS
        .into_iter()
        .find(|s| model_size.x * s <= available_mm.x && model_size.y * s <= available_mm.y)
        .unwrap_or(0.1)
}

/// Converts sheet-space endpoint distance back to model millimetres.
pub fn dimension_value_mm(a: DVec2, b: DVec2, view_scale: f64) -> f64 {
    a.distance(b) / view_scale.max(f64::EPSILON)
}

/// Formats a scale using reciprocal notation.
pub fn scale_label(scale: f64) -> String {
    format!("1:{:.0}", 1.0 / scale)
}

/// Writes one SVG per sheet (`-p2`, `-p3`, ... for later pages).
pub fn export_svg(
    path: &Path,
    drawing: &Drawing,
    projections: &HashMap<u64, ProjectedView>,
    bom_rows: &[BomRow],
) -> Result<(), String> {
    for (index, _) in drawing.sheets.iter().enumerate() {
        let page_path = if index == 0 {
            path.to_path_buf()
        } else {
            page_path(path, index + 1)
        };
        let svg = svg_sheet_string(&drawing.sheets[index], projections, bom_rows);
        std::fs::write(&page_path, svg).map_err(|error| {
            crate::i18n::tr2(
                "Could not write {}: {}",
                &page_path.display().to_string(),
                &error.to_string(),
            )
        })?;
    }
    Ok(())
}

fn page_path(path: &Path, page: usize) -> std::path::PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("drawing");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("svg");
    path.with_file_name(format!("{stem}-p{page}.{extension}"))
}

/// Produces SVG for the active sheet.
#[cfg(test)]
pub fn svg_string(
    drawing: &Drawing,
    projections: &HashMap<u64, ProjectedView>,
    bom_rows: &[BomRow],
) -> String {
    svg_sheet_string(drawing.sheet(), projections, bom_rows)
}

fn svg_sheet_string(
    sheet: &Sheet,
    projections: &HashMap<u64, ProjectedView>,
    bom_rows: &[BomRow],
) -> String {
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{SHEET_WIDTH_MM}mm" height="{SHEET_HEIGHT_MM}mm" viewBox="0 0 {SHEET_WIDTH_MM} {SHEET_HEIGHT_MM}"><rect width="100%" height="100%" fill="white"/><g fill="none" stroke="#111" stroke-width="0.35">"##
    );
    for view in &sheet.views {
        let Some(projected) = projections.get(&view.id) else {
            continue;
        };
        for line in &projected.visible {
            write_polyline(&mut svg, line, view, projected.center, "");
        }
        if view.show_hidden {
            for line in &projected.hidden {
                write_polyline(
                    &mut svg,
                    line,
                    view,
                    projected.center,
                    " stroke=\"#888\" stroke-dasharray=\"2 1\"",
                );
            }
        }
        for segment in &projected.hatch {
            write_polyline(
                &mut svg,
                segment,
                view,
                projected.center,
                " stroke-width=\"0.2\"",
            );
        }
        write_view_annotations_svg(&mut svg, view, projected);
    }
    for dim in &sheet.dims {
        write_dimension_svg(&mut svg, dim);
    }
    for table in &sheet.bom_tables {
        write_bom_svg(&mut svg, table.at, bom_rows);
    }
    for balloon in &sheet.balloons {
        if let Some(number) = bom_number(bom_rows, balloon.body_id) {
            let _ = write!(
                svg,
                "<path d=\"M{} {}L{} {}\"/><circle cx=\"{}\" cy=\"{}\" r=\"4\"/><text x=\"{}\" y=\"{}\" fill=\"#111\" stroke=\"none\" font-size=\"3.5\" text-anchor=\"middle\">{}</text>",
                balloon.anchor.x,
                balloon.anchor.y,
                balloon.at.x,
                balloon.at.y,
                balloon.at.x,
                balloon.at.y,
                balloon.at.x,
                balloon.at.y + 1.2,
                number
            );
        }
    }
    write_title_svg(&mut svg, &sheet.title);
    svg.push_str("</g></svg>");
    svg
}

fn write_bom_svg(svg: &mut String, at: DVec2, rows: &[BomRow]) {
    const WIDTHS: [f64; 5] = [10.0, 32.0, 22.0, 34.0, 10.0];
    let height = 7.0 * (rows.len() + 1) as f64;
    let width: f64 = WIDTHS.iter().sum();
    let _ = write!(
        svg,
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
        at.x, at.y, width, height
    );
    let mut x = at.x;
    for width in WIDTHS.into_iter().take(4) {
        x += width;
        let _ = write!(svg, "<path d=\"M{} {}V{}\"/>", x, at.y, at.y + height);
    }
    for index in 1..=rows.len() {
        let y = at.y + index as f64 * 7.0;
        let _ = write!(svg, "<path d=\"M{} {}H{}\"/>", at.x, y, at.x + width);
    }
    let mut labels = vec![
        (crate::i18n::t("Item No.").to_owned(), at.x + 5.0),
        (crate::i18n::t("Name").to_owned(), at.x + 26.0),
        (crate::i18n::t("Material").to_owned(), at.x + 53.0),
        (crate::i18n::t("Volume").to_owned(), at.x + 81.0),
        (crate::i18n::t("Quantity").to_owned(), at.x + 103.0),
    ];
    for row in rows {
        labels.extend([
            (row.number.to_string(), at.x + 5.0),
            (row.name.clone(), at.x + 26.0),
            (row.material.clone(), at.x + 53.0),
            (row.volume_label.clone(), at.x + 81.0),
            (row.quantity.to_string(), at.x + 103.0),
        ]);
    }
    let _ = write!(
        svg,
        "<g fill=\"#111\" stroke=\"none\" font-size=\"3\" text-anchor=\"middle\">"
    );
    for (index, (label, x)) in labels.into_iter().enumerate() {
        let y = at.y + (index / 5) as f64 * 7.0 + 4.6;
        let _ = write!(
            svg,
            "<text x=\"{}\" y=\"{}\">{}</text>",
            x,
            y,
            escape_xml(&label)
        );
    }
    svg.push_str("</g>");
}

fn write_polyline(
    svg: &mut String,
    line: &[DVec2],
    view: &DrawingView,
    center: DVec2,
    extra: &str,
) {
    if line.len() < 2 {
        return;
    }
    svg.push_str("<path d=\"M");
    for (i, p) in line.iter().enumerate() {
        let p = view.at + (*p - center) * view.scale;
        let _ = write!(svg, "{}{} {}", if i == 0 { "" } else { " L" }, p.x, p.y);
    }
    let _ = write!(svg, "\"{extra}/>");
}

fn write_view_annotations_svg(svg: &mut String, view: &DrawingView, projected: &ProjectedView) {
    if view.show_centerlines {
        for line in &projected.visible {
            if let Some((center, radius)) = detect_circle(line) {
                let c = view.at + (center - projected.center) * view.scale;
                let r = radius * view.scale + 2.0;
                let _ = write!(
                    svg,
                    "<path stroke=\"#555\" stroke-dasharray=\"4 1 1 1\" d=\"M{} {}L{} {}M{} {}L{} {}\"/>",
                    c.x - r,
                    c.y,
                    c.x + r,
                    c.y,
                    c.x,
                    c.y - r,
                    c.x,
                    c.y + r
                );
            }
        }
    }
    match &view.kind {
        ViewKind::Section {
            line_a,
            line_b,
            label,
            ..
        } => {
            let _ = write!(
                svg,
                "<path stroke-width=\"0.7\" d=\"M{} {}L{} {}\"/><g fill=\"#111\" stroke=\"none\" font-size=\"3.5\"><text x=\"{}\" y=\"{}\">{}</text><text x=\"{}\" y=\"{}\">{}</text><text x=\"{}\" y=\"{}\">{} {}-{}</text></g>",
                line_a.x,
                line_a.y,
                line_b.x,
                line_b.y,
                line_a.x + 2.0,
                line_a.y - 2.0,
                label,
                line_b.x + 2.0,
                line_b.y - 2.0,
                label,
                view.at.x,
                view.at.y + projected.size.y * view.scale * 0.5 + 7.0,
                crate::i18n::t("Section"),
                label,
                label
            );
        }
        ViewKind::Detail {
            center,
            radius,
            label,
            ..
        } => {
            let detail_radius = projected.size.max_element() * view.scale * 0.5 + 3.0;
            let _ = write!(
                svg,
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" stroke-dasharray=\"3 2\"/><circle cx=\"{}\" cy=\"{}\" r=\"{}\"/><text x=\"{}\" y=\"{}\" fill=\"#111\" stroke=\"none\" font-size=\"3.5\">{} {}</text>",
                center.x,
                center.y,
                radius,
                view.at.x,
                view.at.y,
                detail_radius,
                view.at.x,
                view.at.y + projected.size.y * view.scale * 0.5 + 7.0,
                crate::i18n::t("Detail"),
                label
            );
        }
        ViewKind::Standard => {
            let _ = write!(
                svg,
                "</g><text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-size=\"3.5\" text-anchor=\"middle\">{} {}</text><g fill=\"none\" stroke=\"#111\" stroke-width=\"0.35\">",
                view.at.x,
                view.at.y + projected.size.y * view.scale * 0.5 + 7.0,
                view.projection.label(),
                scale_label(view.scale)
            );
        }
    }
}

/// Offset dimension-line endpoints.
pub fn dimension_points(dim: &DrawingDim) -> (DVec2, DVec2) {
    let direction = (dim.b - dim.a).normalize_or_zero();
    let normal = DVec2::new(-direction.y, direction.x);
    (dim.a + normal * dim.offset, dim.b + normal * dim.offset)
}

fn write_dimension_svg(svg: &mut String, dim: &DrawingDim) {
    if dim.kind == DimensionKind::Angle
        && let Some(arc) = angle_dimension_arc(dim)
    {
        let _ = write!(
            svg,
            "<path d=\"M{} {}L{} {}",
            dim.a.x, dim.a.y, dim.b.x, dim.b.y
        );
        if let (Some(c), Some(d)) = (dim.c, dim.d) {
            let _ = write!(svg, "M{} {}L{} {}", c.x, c.y, d.x, d.y);
        }
        for (index, point) in arc.iter().enumerate() {
            let _ = write!(
                svg,
                "{}{} {}",
                if index == 0 { "M" } else { "L" },
                point.x,
                point.y
            );
        }
        let label = arc[arc.len() / 2];
        let _ = write!(
            svg,
            "\"/><text x=\"{}\" y=\"{}\" fill=\"#111\" stroke=\"none\" font-size=\"3.5\" text-anchor=\"middle\">{}</text>",
            label.x,
            label.y - 1.2,
            dim.label()
        );
        return;
    }
    let (a, b) = dimension_points(dim);
    let direction = (b - a).normalize_or_zero();
    let normal = DVec2::new(-direction.y, direction.x);
    let _ = write!(
        svg,
        "<path d=\"M{} {}L{} {}M{} {}L{} {}M{} {}L{} {}M{} {}L{} {}M{} {}L{} {}M{} {}L{} {}M{} {}L{} {}\"/><text x=\"{}\" y=\"{}\" fill=\"#111\" stroke=\"none\" font-size=\"3.5\" text-anchor=\"middle\">{}</text>",
        dim.a.x,
        dim.a.y,
        a.x,
        a.y,
        dim.b.x,
        dim.b.y,
        b.x,
        b.y,
        a.x,
        a.y,
        b.x,
        b.y,
        a.x,
        a.y,
        (a + direction * 3.0 + normal * 1.2).x,
        (a + direction * 3.0 + normal * 1.2).y,
        a.x,
        a.y,
        (a + direction * 3.0 - normal * 1.2).x,
        (a + direction * 3.0 - normal * 1.2).y,
        b.x,
        b.y,
        (b - direction * 3.0 + normal * 1.2).x,
        (b - direction * 3.0 + normal * 1.2).y,
        b.x,
        b.y,
        (b - direction * 3.0 - normal * 1.2).x,
        (b - direction * 3.0 - normal * 1.2).y,
        (a.x + b.x) * 0.5,
        (a.y + b.y) * 0.5 - 1.2,
        dim.label()
    );
}

fn write_title_svg(svg: &mut String, title: &TitleBlock) {
    let _ = write!(
        svg,
        "<rect x=\"177\" y=\"172\" width=\"115\" height=\"33\"/><path d=\"M177 183H292M177 194H292M220 172V205M260 183V205\"/><g fill=\"#111\" stroke=\"none\" font-size=\"3\"><text x=\"179\" y=\"177\">{} {}</text><text x=\"179\" y=\"188\">{} {}</text><text x=\"222\" y=\"188\">{} {}</text><text x=\"262\" y=\"188\">{} {}</text><text x=\"179\" y=\"199\">{} {}</text><text x=\"222\" y=\"199\">{} {}</text></g>",
        crate::i18n::t("Project Name"),
        escape_xml(&title.project_name),
        crate::i18n::t("Drawing Number"),
        escape_xml(&title.drawing_number),
        crate::i18n::t("Drawing Scale"),
        escape_xml(&title.scale),
        crate::i18n::t("Units"),
        escape_xml(&title.units),
        crate::i18n::t("Date"),
        escape_xml(&title.date),
        crate::i18n::t("Author"),
        escape_xml(&title.author)
    );
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Writes a dependency-free multi-page PDF 1.4 vector drawing.
pub fn export_pdf(
    path: &Path,
    drawing: &Drawing,
    projections: &HashMap<u64, ProjectedView>,
    bom_rows: &[BomRow],
) -> Result<(), String> {
    std::fs::write(path, pdf_bytes(drawing, projections, bom_rows)).map_err(|error| {
        crate::i18n::tr2(
            "Could not write {}: {}",
            &path.display().to_string(),
            &error.to_string(),
        )
    })
}

/// Produces a minimal multi-page PDF with a valid xref table.
pub fn pdf_bytes(
    drawing: &Drawing,
    projections: &HashMap<u64, ProjectedView>,
    bom_rows: &[BomRow],
) -> Vec<u8> {
    const PT: f64 = 72.0 / 25.4;
    let streams = drawing
        .sheets
        .iter()
        .map(|sheet| {
            let mut out = String::from("0.35 w 0 G\n");
            for view in &sheet.views {
                if let Some(p) = projections.get(&view.id) {
                    for line in &p.visible {
                        pdf_line(&mut out, line, view, p.center, PT);
                    }
                    for h in &p.hatch {
                        pdf_line(&mut out, h, view, p.center, PT);
                    }
                    if view.show_hidden {
                        out.push_str("[5 3] 0 d 0.6 G\n");
                        for line in &p.hidden {
                            pdf_line(&mut out, line, view, p.center, PT);
                        }
                        out.push_str("[] 0 d 0 G\n");
                    }
                    if view.show_centerlines {
                        for line in &p.visible {
                            if let Some((center, radius)) = detect_circle(line) {
                                let center = view.at + (center - p.center) * view.scale;
                                let radius = radius * view.scale + 2.0;
                                pdf_sheet_segment(&mut out, center-DVec2::X*radius, center+DVec2::X*radius, PT);
                                pdf_sheet_segment(&mut out, center-DVec2::Y*radius, center+DVec2::Y*radius, PT);
                            }
                        }
                    }
                    match &view.kind {
                        ViewKind::Section { line_a, line_b, .. } => pdf_sheet_segment(&mut out,*line_a,*line_b,PT),
                        ViewKind::Detail { center, radius, .. } => {
                            pdf_circle(&mut out,*center,*radius,PT);
                            pdf_circle(&mut out,view.at,p.size.max_element()*view.scale*0.5+3.0,PT);
                        }
                        ViewKind::Standard => {}
                    }
                }
            }
            for dim in &sheet.dims {
                if dim.kind == DimensionKind::Angle
                    && let Some(arc)=angle_dimension_arc(dim)
                {
                    if let (Some(c),Some(d))=(dim.c,dim.d){pdf_sheet_segment(&mut out,dim.a,dim.b,PT);pdf_sheet_segment(&mut out,c,d,PT);}
                    for edge in arc.windows(2){pdf_sheet_segment(&mut out,edge[0],edge[1],PT);}
                    let label=arc[arc.len()/2];let _=writeln!(out,"BT /F1 9 Tf {} {} Td ({}) Tj ET",label.x*PT,(SHEET_HEIGHT_MM-label.y)*PT,pdf_escape(&dim.label()));
                    continue;
                }
                let (a, b) = dimension_points(dim);
                let _ = writeln!(
                    out,
                    "{} {} m {} {} l S BT /F1 9 Tf {} {} Td ({}) Tj ET",
                    a.x * PT,
                    (SHEET_HEIGHT_MM - a.y) * PT,
                    b.x * PT,
                    (SHEET_HEIGHT_MM - b.y) * PT,
                    (a.x + b.x) * 0.5 * PT,
                    (SHEET_HEIGHT_MM - (a.y + b.y) * 0.5) * PT,
                    pdf_escape(&dim.label())
                );
            }
            for table in &sheet.bom_tables {
                pdf_bom(&mut out, table.at, bom_rows, PT);
            }
            for balloon in &sheet.balloons {
                if let Some(number) = bom_number(bom_rows, balloon.body_id) {
                    pdf_sheet_segment(&mut out, balloon.anchor, balloon.at, PT);
                    pdf_circle(&mut out, balloon.at, 4.0, PT);
                    let _ = writeln!(out, "BT /F1 9 Tf {} {} Td ({}) Tj ET", (balloon.at.x - 1.0) * PT, (SHEET_HEIGHT_MM - balloon.at.y - 1.0) * PT, number);
                }
            }
            let _ = writeln!(
                out,
                "{} {} {} {} re S {} {} m {} {} l {} {} m {} {} l {} {} m {} {} l S BT /F1 8 Tf {} {} Td (Project: {}) Tj ET BT /F1 8 Tf {} {} Td (No: {}  Scale: {}  Units: {}) Tj ET BT /F1 8 Tf {} {} Td (Date: {}  Author: {}) Tj ET",
                177.0 * PT,
                5.0 * PT,
                115.0 * PT,
                33.0 * PT,
                177.0*PT,16.0*PT,292.0*PT,16.0*PT,
                177.0*PT,27.0*PT,292.0*PT,27.0*PT,
                220.0*PT,5.0*PT,220.0*PT,27.0*PT,
                179.0 * PT,
                31.0 * PT,
                pdf_escape(&sheet.title.project_name),
                179.0*PT,20.0*PT,pdf_escape(&sheet.title.drawing_number),pdf_escape(&sheet.title.scale),pdf_escape(&sheet.title.units),
                179.0*PT,9.0*PT,pdf_escape(&sheet.title.date),pdf_escape(&sheet.title.author)
            );
            out.into_bytes()
        })
        .collect::<Vec<_>>();
    build_pdf(&streams)
}

fn pdf_bom(out: &mut String, at: DVec2, rows: &[BomRow], pt: f64) {
    let widths = [10.0, 32.0, 22.0, 34.0, 10.0];
    let width: f64 = widths.iter().sum();
    let height = 7.0 * (rows.len() + 1) as f64;
    let _ = writeln!(
        out,
        "{} {} {} {} re S",
        at.x * pt,
        (SHEET_HEIGHT_MM - at.y - height) * pt,
        width * pt,
        height * pt
    );
    let mut x = at.x;
    for cell in widths.into_iter().take(4) {
        x += cell;
        pdf_sheet_segment(out, DVec2::new(x, at.y), DVec2::new(x, at.y + height), pt);
    }
    for index in 1..=rows.len() {
        let y = at.y + index as f64 * 7.0;
        pdf_sheet_segment(out, DVec2::new(at.x, y), DVec2::new(at.x + width, y), pt);
    }
    for (row_index, cells) in std::iter::once(vec![
        "No".to_owned(),
        "Name".to_owned(),
        "Material".to_owned(),
        "Volume".to_owned(),
        "Qty".to_owned(),
    ])
    .chain(rows.iter().map(|row| {
        vec![
            row.number.to_string(),
            row.name.clone(),
            row.material.clone(),
            row.volume_label.clone(),
            row.quantity.to_string(),
        ]
    }))
    .enumerate()
    {
        let mut x = at.x;
        for (cell, width) in cells.into_iter().zip(widths) {
            let _ = writeln!(
                out,
                "BT /F1 7 Tf {} {} Td ({}) Tj ET",
                (x + 1.0) * pt,
                (SHEET_HEIGHT_MM - at.y - row_index as f64 * 7.0 - 4.8) * pt,
                pdf_escape(&cell)
            );
            x += width;
        }
    }
}

fn pdf_sheet_segment(out: &mut String, a: DVec2, b: DVec2, pt: f64) {
    let _ = writeln!(
        out,
        "{} {} m {} {} l S",
        a.x * pt,
        (SHEET_HEIGHT_MM - a.y) * pt,
        b.x * pt,
        (SHEET_HEIGHT_MM - b.y) * pt
    );
}

fn pdf_circle(out: &mut String, center: DVec2, radius: f64, pt: f64) {
    let points = (0..=48)
        .map(|i| {
            let angle = i as f64 / 48.0 * std::f64::consts::TAU;
            center + DVec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect::<Vec<_>>();
    for (i, p) in points.iter().enumerate() {
        let _ = writeln!(
            out,
            "{} {} {}",
            p.x * pt,
            (SHEET_HEIGHT_MM - p.y) * pt,
            if i == 0 { "m" } else { "l" }
        );
    }
    out.push_str("S\n");
}

fn pdf_line(out: &mut String, line: &[DVec2], view: &DrawingView, center: DVec2, pt: f64) {
    if line.len() < 2 {
        return;
    }
    for (i, p) in line.iter().enumerate() {
        let p = view.at + (*p - center) * view.scale;
        let _ = writeln!(
            out,
            "{} {} {}",
            p.x * pt,
            (SHEET_HEIGHT_MM - p.y) * pt,
            if i == 0 { "m" } else { "l" }
        );
    }
    out.push_str("S\n");
}
fn pdf_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

fn build_pdf(streams: &[Vec<u8>]) -> Vec<u8> {
    let page_count = streams.len();
    let font_id = 3 + page_count * 2;
    let mut objects = Vec::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids = (0..page_count)
        .map(|i| format!("{} 0 R", 3 + i * 2))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes());
    for (i, stream) in streams.iter().enumerate() {
        let content = 4 + i * 2;
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 {font_id} 0 R >> >> /Contents {content} 0 R >>",SHEET_WIDTH_MM*72.0/25.4,SHEET_HEIGHT_MM*72.0/25.4).into_bytes());
        objects.push(
            [
                format!("<< /Length {} >>\nstream\n", stream.len()).as_bytes(),
                stream,
                b"endstream",
            ]
            .concat(),
        );
    }
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(obj);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{document::Document, history::PrimitiveKind};

    #[test]
    fn angle_math() {
        assert_eq!(
            angle_degrees(DVec2::ZERO, DVec2::X, DVec2::ZERO, DVec2::Y),
            Some(90.0)
        );
        assert!(
            (angle_degrees(DVec2::ZERO, DVec2::X, DVec2::ZERO, DVec2::ONE).unwrap() - 45.0).abs()
                < 1e-9
        );
    }

    #[test]
    fn circle_detection() {
        let line = (0..=64)
            .map(|i| {
                let a = i as f64 / 64.0 * std::f64::consts::TAU;
                DVec2::new(4.0, -2.0) + DVec2::new(a.cos(), a.sin()) * 12.0
            })
            .collect::<Vec<_>>();
        let (center, radius) = detect_circle(&line).unwrap();
        assert!(center.distance(DVec2::new(4.0, -2.0)) < 1e-6);
        assert!((radius - 12.0).abs() < 1e-6);
    }

    #[test]
    fn w10_migrates_and_drawing_undo_restores_view_count() {
        let old = r#"{"views":[{"id":1,"source":"AllBodies","projection":"Front","at":[100.0,80.0],"scale":1.0,"show_hidden":false}],"dims":[{"a":[10.0,10.0],"b":[20.0,10.0],"offset":8.0,"value_mm":10.0}],"next_view_id":2}"#;
        let mut drawing: Drawing = serde_json::from_str(old).unwrap();
        assert_eq!(drawing.sheets.len(), 1);
        assert!(!drawing.sheet().title.date.is_empty());
        assert_eq!(drawing.sheet().views.len(), 1);
        assert!(drawing.sheet().views[0].show_centerlines);
        assert_eq!(drawing.sheet().dims[0].kind, DimensionKind::Linear);
        drawing.add_view(Projection::Front, DVec2::ZERO, 1.0);
        assert_eq!(drawing.sheet().views.len(), 2);
        assert!(drawing.undo());
        assert_eq!(drawing.sheet().views.len(), 1);
    }

    #[test]
    fn title_and_multi_sheet_roundtrip() {
        let mut d = Drawing::default();
        d.sheet_mut().title.author = crate::i18n::t("Test").into();
        d.add_sheet();
        d.sheet_mut().title.drawing_number = "002".into();
        let loaded: Drawing = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(loaded.sheets.len(), 2);
        assert_eq!(loaded.sheets[0].title.author, crate::i18n::t("Test"));
        assert_eq!(loaded.sheets[1].title.drawing_number, "002");
    }

    #[test]
    fn hatch_segments_are_inside_outline_bbox() {
        let outline = vec![vec![
            DVec2::ZERO,
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(0.0, 10.0),
            DVec2::ZERO,
        ]];
        let hatch = hatch_outline(&outline, 2.0);
        assert!(!hatch.is_empty());
        assert!(
            hatch
                .iter()
                .flatten()
                .all(|p| p.x >= 0.0 && p.x <= 10.0 && p.y >= 0.0 && p.y <= 10.0)
        );
    }

    #[test]
    fn section_box_hlr_is_clipped_and_hatched() {
        let shape = Shape::cube(20.0).unwrap();
        let full = shape_hlr(&shape, Projection::Front).unwrap();
        let section =
            shape_section_hlr(&shape, DVec3::new(0.0, 0.0, 10.0), DVec3::Z, DVec3::Z).unwrap();
        assert!(section.visible.len() <= full.visible.len());
        assert!(!section.hatch.is_empty());
        let min = section
            .section_outline
            .iter()
            .flatten()
            .fold(DVec2::splat(f64::INFINITY), |a, p| a.min(*p));
        let max = section
            .section_outline
            .iter()
            .flatten()
            .fold(DVec2::splat(f64::NEG_INFINITY), |a, p| a.max(*p));
        assert!(
            section
                .hatch
                .iter()
                .flatten()
                .all(|p| p.cmpge(min - DVec2::splat(1e-6)).all()
                    && p.cmple(max + DVec2::splat(1e-6)).all())
        );
    }

    #[test]
    fn cylinder_top_projection_contains_detectable_circle() {
        let shape = Shape::cylinder(DVec3::ZERO, 12.0, DVec3::Z, 20.0).unwrap();
        let projected = shape_hlr(&shape, Projection::Top).unwrap();
        assert!(
            projected
                .visible
                .iter()
                .any(|line| detect_circle(line).is_some())
        );
    }

    #[test]
    fn svg_contains_title_block() {
        let drawing = Drawing::default();
        let svg = svg_string(&drawing, &HashMap::new(), &[]);
        assert!(svg.contains(crate::i18n::t("Project Name")));
        assert!(svg.contains("DUCTILE-001"));
    }

    #[test]
    fn multi_sheet_vector_export_has_pages_and_svg_suffix() {
        let directory = tempfile::tempdir().unwrap();
        let mut drawing = Drawing::default();
        drawing.add_sheet();
        drawing.active_sheet = 0;
        let svg = directory.path().join("drawing.svg");
        export_svg(&svg, &drawing, &HashMap::new(), &[]).unwrap();
        assert!(svg.exists());
        assert!(directory.path().join("drawing-p2.svg").exists());
        let pdf = String::from_utf8_lossy(&pdf_bytes(&drawing, &HashMap::new(), &[])).into_owned();
        assert!(pdf.contains("/Count 2"));
    }

    #[test]
    fn bom_and_balloon_are_emitted_to_vector_exports() {
        let mut drawing = Drawing::default();
        drawing.sheet_mut().bom_tables.push(BomTable {
            at: DVec2::new(10.0, 10.0),
        });
        drawing.sheet_mut().balloons.push(Balloon {
            view_id: 1,
            body_id: BodyId(7),
            anchor: DVec2::new(20.0, 20.0),
            at: DVec2::new(30.0, 10.0),
        });
        let rows = vec![BomRow {
            number: 1,
            name: "Part".into(),
            material: "Metal".into(),
            volume: 12.0,
            volume_label: "12.000 mm³".into(),
            quantity: 1,
            bodies: vec![BodyId(7)],
        }];
        let svg = svg_string(&drawing, &HashMap::new(), &rows);
        assert!(svg.contains("Part"));
        assert!(svg.contains("<circle cx=\"30\" cy=\"10\""));
        let bytes = pdf_bytes(&drawing, &HashMap::new(), &rows);
        let pdf = String::from_utf8_lossy(&bytes);
        assert!(pdf.contains("(Part)"));
        assert!(pdf.contains("(1) Tj"));
    }

    #[test]
    fn bom_groups_identical_boxes_and_counts_two() {
        let mut document = Document::new();
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::new(10.0, 20.0, 30.0),
        });
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::new(50.0, 0.0, 0.0),
            max: DVec3::new(60.0, 20.0, 30.0),
        });
        let rows = bom_rows(&document.bodies, Units::Millimeter);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].quantity, 2);
        assert_eq!(rows[0].bodies.len(), 2);
    }

    #[test]
    fn bom_volume_column_converts_to_selected_units() {
        let mut document = Document::new();
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let rows = bom_rows(&document.bodies, Units::Centimeter);
        assert!((rows[0].volume - 1.0).abs() < 1.0e-9);
        assert!(rows[0].volume_label.ends_with("cm³"));
    }

    #[test]
    fn balloon_resolves_body_from_tagged_polyline() {
        let view = DrawingView {
            id: 1,
            source: DrawingSource::AllBodies,
            projection: Projection::Front,
            view_dir: None,
            at: DVec2::new(50.0, 50.0),
            scale: 1.0,
            show_hidden: false,
            show_centerlines: false,
            kind: ViewKind::Standard,
        };
        let mut projected = projected_view(
            vec![vec![DVec2::new(-10.0, 0.0), DVec2::new(10.0, 0.0)]],
            Vec::new(),
        );
        projected.visible_sources = vec![Some(BodyId(42))];
        assert_eq!(
            resolve_balloon_body(&view, &projected, DVec2::new(55.0, 51.0), 2.0),
            Some(BodyId(42))
        );
    }
}
