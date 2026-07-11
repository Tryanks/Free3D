//! Lightweight process-wide localization for Free3D user-facing copy.
//!
//! English source strings are stable lookup keys. The selected language is an
//! atomic value so a Settings change is visible to the next render without an
//! application restart.

use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
};

use serde::{Deserialize, Serialize};

/// A language supported by the Free3D interface.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Lang {
    /// English, the source and fallback language.
    En = 0,
    /// Simplified Chinese.
    ZhCn = 1,
}

/// Persisted language preference.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LangChoice {
    /// Detect from `FREE3D_LANG`, then the operating-system locale.
    #[default]
    Auto,
    /// Always use English.
    En,
    /// Always use Simplified Chinese.
    ZhCn,
}

/// Native name shown for Simplified Chinese in every interface language.
pub const ZH_ENDONYM: &str = "简体中文";

static CURRENT_LANG: AtomicU8 = AtomicU8::new(Lang::En as u8);
static ZH_LOOKUP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

/// Returns the process-wide interface language.
pub fn lang() -> Lang {
    match CURRENT_LANG.load(Ordering::Relaxed) {
        1 => Lang::ZhCn,
        _ => Lang::En,
    }
}

/// Changes the process-wide interface language.
pub fn set_lang(value: Lang) {
    CURRENT_LANG.store(value as u8, Ordering::Relaxed);
}

/// Resolves and installs the startup language.
pub fn init(choice: LangChoice) {
    let resolved = match choice {
        LangChoice::En => Lang::En,
        LangChoice::ZhCn => Lang::ZhCn,
        LangChoice::Auto => std::env::var("FREE3D_LANG")
            .ok()
            .and_then(|value| lang_from_locale_str(&value))
            .or_else(|| sys_locale::get_locale().and_then(|value| lang_from_locale_str(&value)))
            .unwrap_or(Lang::En),
    };
    set_lang(resolved);
}

/// Parses a locale identifier using its language prefix.
pub fn lang_from_locale_str(locale: &str) -> Option<Lang> {
    let prefix = locale
        .trim()
        .split(['-', '_', '.'])
        .next()
        .unwrap_or_default();
    if prefix.eq_ignore_ascii_case("zh") {
        Some(Lang::ZhCn)
    } else if prefix.eq_ignore_ascii_case("en") {
        Some(Lang::En)
    } else {
        None
    }
}

/// Translates one English source string, falling back to it when untranslated.
pub fn t(en: &'static str) -> &'static str {
    translate(lang(), en)
}

/// Returns an English source key for a displayed translation when available.
pub fn english_key(displayed: &'static str) -> &'static str {
    if lang() == Lang::En {
        return displayed;
    }
    ZH_TRANSLATIONS
        .iter()
        .find_map(|&(en, zh)| (zh == displayed).then_some(en))
        .unwrap_or(displayed)
}

fn translate(language: Lang, en: &'static str) -> &'static str {
    if language == Lang::En {
        return en;
    }
    zh_lookup().get(en).copied().unwrap_or(en)
}

/// Translates a source key for an explicit language without changing the
/// process-wide preference. This is also useful for deterministic formatters.
#[cfg(test)]
pub fn translate_for(language: Lang, en: &'static str) -> &'static str {
    translate(language, en)
}

/// Translates a template and replaces its first `{}` placeholder.
pub fn tr1(key: &'static str, arg: &str) -> String {
    replace_one(t(key), arg)
}

/// Translates a template and replaces its first two `{}` placeholders.
pub fn tr2(key: &'static str, first: &str, second: &str) -> String {
    replace_one(&replace_one(t(key), first), second)
}

fn replace_one(template: &str, arg: &str) -> String {
    template.replacen("{}", arg, 1)
}

fn zh_lookup() -> &'static HashMap<&'static str, &'static str> {
    ZH_LOOKUP.get_or_init(|| ZH_TRANSLATIONS.iter().copied().collect())
}

static ZH_TRANSLATIONS: &[(&str, &str)] = &[
    ("Auto", "自动"),
    ("Language", "语言"),
    ("Home", "主页"),
    ("Sync", "同步"),
    ("Share", "分享"),
    ("Settings", "设置"),
    ("Help", "帮助"),
    ("Theme", "主题"),
    ("Navigation Preset", "导航预设"),
    ("Units", "单位"),
    (
        "Autosave interval (seconds, 0 = off)",
        "自动保存间隔（秒，0 = 关闭）",
    ),
    ("Files", "文件"),
    ("Import", "导入"),
    ("Export", "导出"),
    ("Recent Files", "最近打开"),
    ("Project", "工程"),
    ("Dark", "深色"),
    ("Light", "浅色"),
    ("Modeling", "建模"),
    ("Visualize", "可视化"),
    ("Drawing", "绘图"),
    ("Items", "项目"),
    ("Selected Items", "项"),
    ("Faces", "面"),
    ("Edges", "边"),
    ("Bodies", "体"),
    ("Deselect All", "全部取消选择"),
    ("Total", "总计"),
    ("Variables", "变量"),
    ("Materials", "材质"),
    (
        "Select a body to edit its material",
        "选择一个实体以编辑材质",
    ),
    ("Original", "原色"),
    ("Metal", "金属"),
    ("Plastic", "塑料"),
    ("Glass", "玻璃感"),
    ("Hue", "H 色相"),
    ("Saturation", "S 饱和度"),
    ("Lightness", "L 明度"),
    ("Section", "剖视"),
    ("Section View", "剖视图"),
    ("Detail", "局部放大"),
    ("Radius", "半径"),
    ("Diameter", "直径"),
    ("Angle", "角度"),
    ("Parts List", "明细表"),
    ("Balloon", "气泡"),
    ("Tools", "工具"),
    ("Sketch", "草图"),
    ("Add", "添加"),
    ("Transform", "变换"),
    ("Line", "直线"),
    ("Rectangle", "矩形"),
    ("Center Rectangle", "中心矩形"),
    ("Rounded Rectangle", "圆角矩形"),
    ("Polygon", "多边形"),
    ("Slot", "槽"),
    ("Circle", "圆"),
    ("Three-Point Circle", "三点圆"),
    ("Ellipse", "椭圆"),
    ("Elliptical Arc", "椭圆弧"),
    ("Arc", "圆弧"),
    ("Point", "点"),
    ("Tangent Arc", "切线弧"),
    ("Spline", "样条"),
    ("Control-Point Spline", "控制点样条"),
    ("Two-Tangent Circle", "两切线圆"),
    ("Three-Tangent Circle", "三切线圆"),
    ("Sketch Fillet", "草图圆角"),
    ("Trim", "修剪"),
    ("Extend", "延伸"),
    ("Break", "打断"),
    ("Sketch Offset", "草图偏移"),
    ("Box", "长方体"),
    ("Cylinder", "圆柱体"),
    ("Sphere", "球体"),
    ("Cone", "圆锥体"),
    ("Torus", "圆环体"),
    ("Ellipsoid", "椭球体"),
    ("Prism", "棱柱"),
    ("Wedge", "楔形"),
    ("Construction Plane", "构造平面"),
    ("Construction Axis", "构造轴"),
    ("Construction Point", "构造点"),
    ("Reference Image", "参考图像"),
    ("Helix", "螺旋线"),
    ("Thread", "螺纹"),
    ("Move/Rotate", "移动/旋转"),
    ("Translate", "平移"),
    ("Scale", "缩放"),
    ("Mirror", "镜像"),
    ("Pattern", "阵列"),
    ("Align", "对齐"),
    ("Ground", "接地"),
    ("Joint", "关节"),
    ("Drive", "驱动"),
    ("Extrude", "拉伸"),
    ("Revolve", "旋转体"),
    ("Sweep", "扫掠"),
    ("Loft", "放样"),
    ("Patch", "修补"),
    ("Stitch", "缝合"),
    ("Thicken", "加厚"),
    ("Delete Face", "删除面"),
    ("Shell", "抽壳"),
    ("Fillet", "圆角"),
    ("Chamfer", "倒角"),
    ("Offset Face", "偏移面"),
    ("Replace Face", "替换面"),
    ("Hole", "孔"),
    ("Draft", "拔模"),
    ("Split Body", "分割体"),
    ("Project Geometry", "投影"),
    ("Boolean Union", "布尔并集"),
    ("Boolean Subtract", "布尔减集"),
    ("Boolean Intersect", "布尔交集"),
    ("Properties", "属性"),
    ("Interference Check", "干涉检查"),
    ("Check Geometry", "检查几何"),
    ("Undo", "撤销"),
    ("Redo", "重做"),
    ("Save", "保存"),
    ("Save As", "另存为"),
    ("Open", "打开"),
    ("New Project", "新建项目"),
    ("Isometric View", "等轴测视图"),
    ("Front View", "前视"),
    ("Back View", "后视"),
    ("Top View", "顶视"),
    ("Bottom View", "底视"),
    ("Right View", "右视"),
    ("Left View", "左视"),
    ("Isometric", "等轴测"),
    ("Front", "前"),
    ("Back", "后"),
    ("Top", "顶"),
    ("Bottom", "底"),
    ("Right", "右"),
    ("Left", "左"),
    ("Search commands…", "搜索命令…"),
    ("Construction", "构造"),
    ("Horizontal", "水平"),
    ("Vertical", "垂直"),
    ("Parallel", "平行"),
    ("Perpendicular", "正交"),
    ("Equal", "相等"),
    ("Tangent", "相切"),
    ("Collinear", "共线"),
    ("Curvature Continuous", "曲率连续"),
    ("Lock/Unlock", "锁定/解锁"),
    ("Concentric", "同心"),
    ("Coincident", "重合"),
    ("Symmetric", "对称"),
    ("Point on Object", "点在线上"),
    ("Section View Mode", "剖切视图"),
    ("Isolate", "隔离"),
    ("Measure", "测量"),
    ("Exploded", "爆炸"),
    ("On", "开启"),
    ("Off", "关闭"),
    ("One-Sided", "单向"),
    ("Two-Sided", "双向"),
    ("New Body", "新建体"),
    ("Union", "并集"),
    ("Subtract", "减集"),
    ("Intersect", "交集"),
    ("History", "历史记录"),
    ("Fixed", "固定"),
    ("Revolute", "旋转"),
    ("Slider", "滑动"),
    ("Cylindrical", "圆柱"),
    ("Ball", "球"),
    ("Joints", "关节"),
    ("Joints · Over-constrained", "关节 · 过约束"),
    ("Screenshot", "截图"),
    ("Snap", "捕捉"),
    ("View", "视图"),
    ("Grid Spacing", "网格间距"),
    ("Display Mode", "显示模式"),
    ("Standard Views", "标准视图"),
    ("Surface Analysis", "表面分析"),
    ("Zebra", "斑马纹"),
    ("Curvature", "曲率"),
    ("Field of View", "视场角"),
    ("Grid Plane", "网格平面"),
    ("Save View", "保存视图"),
    ("Shaded", "着色"),
    ("Wireframe", "线框"),
    ("Hidden Lines", "隐藏线"),
    ("Hidden Edges", "隐藏边"),
    ("Through Selection", "穿透选择"),
    ("Confirm", "确认"),
    ("Linear", "线性"),
    ("Circular", "环形"),
    ("Variable", "可变"),
    ("Constant", "恒定"),
    ("None", "无"),
    ("Internal Thread", "内螺纹"),
    ("External Thread", "外螺纹"),
    ("Decorative", "装饰"),
    ("Modeled", "实体"),
    ("Blind", "盲孔"),
    ("Through", "通孔"),
    ("Counterbore", "沉孔"),
    ("Countersink", "锥孔"),
    ("Length", "长度"),
    ("Distance", "距离"),
    ("Depth", "深度"),
    ("Horizontal Distance", "水平距离"),
    ("Vertical Distance", "垂直距离"),
    ("Label", "标注"),
    ("Body", "体"),
    ("Face", "面"),
    ("Edge", "边"),
    ("Reference", "参考"),
    ("Centerline", "中心线"),
    ("Item", "实体"),
    ("Untitled", "未命名"),
    ("Untitled Project", "未命名项目"),
    ("By View", "按视图"),
    ("Default", "默认"),
    ("Rubber", "橡胶"),
    ("Item No.", "序号"),
    ("Name", "名称"),
    ("Material", "材质"),
    ("Volume", "体积"),
    ("Quantity", "数量"),
    ("Project Name", "项目名"),
    ("Drawing Number", "图号"),
    ("Drawing Scale", "比例"),
    ("Date", "日期"),
    ("Author", "作者"),
    ("Millimeter", "毫米"),
    ("Centimeter", "厘米"),
    ("Meter", "米"),
    ("Inch", "英寸"),
    ("mm", "毫米"),
    ("cm", "厘米"),
    ("m", "米"),
    ("in", "英寸"),
    ("Free3D Default", "Free3D 默认"),
    ("Scroll to Zoom", "滚动缩放"),
    ("Blender Style", "Blender 风格"),
    ("Fusion Style", "Fusion 风格"),
    ("SolidWorks Style", "SolidWorks 风格"),
    ("Classic Trackpad", "触控板经典"),
    ("Search", "搜索"),
    ("Cancel", "取消"),
    ("Continue", "继续"),
    ("Restore", "恢复"),
    ("Ignore", "忽略"),
    ("Plan View", "俯视"),
    ("Three points are collinear", "三点共线"),
    ("No body selected", "未选择实体"),
    (
        "Could not uniquely resolve the referenced face",
        "无法唯一定位引用的面",
    ),
    (
        "Could not uniquely resolve the referenced edge",
        "无法唯一定位引用的边",
    ),
    (
        "Select at least two bodies; boolean operations do not support surface bodies",
        "请选择至少两个实体；曲面体不支持布尔运算",
    ),
    ("Select a cylindrical face", "请选择一个圆柱面"),
    (
        "Select one complete closed boundary on the same body",
        "请选择同一实体的一条完整闭合边界",
    ),
    (
        "Select at least two surface bodies to stitch",
        "请选择至少两个曲面体进行缝合",
    ),
    (
        "The deleted face could not heal into a valid body; the model was not changed",
        "删除面无法愈合为有效实体，模型未更改",
    ),
    (
        "Reference image (.png .jpg .jpeg)",
        "参考图像 (.png .jpg .jpeg)",
    ),
    (
        "Drawing export supports only .svg or .pdf",
        "绘图仅支持 .svg 或 .pdf",
    ),
    (
        "Unsaved changes will be lost. Continue?",
        "未保存的更改将丢失，仍要继续？",
    ),
    ("Open Free3D project (.f3d)", "打开 Free3D 工程 (.f3d)"),
    (
        "A newer autosave was found. Restore it?",
        "检测到未保存的自动备份，是否恢复？",
    ),
    (
        "Selected geometry does not support this operation; body-only tools cannot be used on surface bodies",
        "所选几何不支持此操作；实体专用工具不能用于曲面体",
    ),
    (
        "Pick the first connection frame · plane, circular edge, construction axis, or point",
        "拾取第一个连接框架 · 平面/圆边/构造轴或点",
    ),
    (
        "Select a plane, circular edge, construction axis, or point",
        "请选择平面、圆边、构造轴或点",
    ),
    (
        "The second connection frame must belong to a different body",
        "第二个连接框架必须属于不同实体",
    ),
    ("Joint created · Revolute", "关节已创建 · 旋转"),
    (
        "Pick the second connection frame · it must belong to a different body",
        "拾取第二个连接框架 · 必须属于不同实体",
    ),
    ("Sketch mirror created", "草图镜像已创建"),
    (
        "Click a target face or construction plane",
        "点击目标面或构造平面",
    ),
    (
        "Click an edge or face outline to project",
        "点击要投影的边或面轮廓",
    ),
    ("Enter pitch", "输入螺距"),
    ("Click the hole position", "点击孔位置"),
    ("Select the mirror line", "选择镜像线"),
    (
        "Linear pattern: drag direction and spacing",
        "线性阵列：拖动方向和间距",
    ),
    ("Drag or enter depth", "拖动或输入深度"),
    ("Drag or enter diameter", "拖动或输入直径"),
    ("Circular pattern: click the center", "环形阵列：点击中心"),
    (
        "Projection created as construction geometry",
        "投影已创建为构造几何",
    ),
    (
        "Select the required number of nonparallel lines",
        "请选择所需数量的非平行直线",
    ),
    (
        "No intersection was found to extend to",
        "未找到可延伸到的交点",
    ),
    ("Cannot break at this position", "该位置无法打断"),
    (
        "Elliptical arcs cannot be trimmed yet",
        "椭圆弧暂不支持修剪",
    ),
    ("Enter thread depth", "输入螺纹深度"),
    ("Enter turns", "输入圈数"),
    ("Enter wire radius", "输入线径半径"),
    ("Enter end radius", "输入终点半径"),
    ("Tangent Arc · T switches to Line", "切线弧 · T 切换直线"),
    ("Line · T switches to Tangent Arc", "直线 · T 切换切线弧"),
    ("Expression is invalid", "表达式无效"),
    ("Feature expression is invalid", "特征表达式无效"),
    ("Constraint solver did not converge", "约束求解未收敛"),
    ("Recompute failed", "重算失败"),
    ("recompute failed", "重算失败"),
    ("Reference ", "参考 "),
    ("Turn Left", "左旋"),
    ("Turn Right", "右旋"),
    ("Test", "测试"),
    ("Assembly Project", "装配项目"),
    ("Untitled.f3d", "未命名.f3d"),
    ("No interference", "✓ 无干涉"),
    ("Undefined variable missing", "未定义变量 missing"),
    ("Page {}", "页 {}"),
    ("View {}", "视图 {}"),
    ("{} sides", "{} 边"),
    ("[−] {} sides [+]", "[−] {} 边 [+]"),
    ("Target face: {}", "目标面：{}"),
    ("Axis: {} · enter radius", "轴：{} · 输入半径"),
    ("Undefined variable {}", "未定义变量 {}"),
    (
        "Feature expression references undefined variable {}",
        "特征表达式引用未定义变量 {}",
    ),
    (
        "History step {} recompute failed: {}",
        "历史步骤 {} 重算失败：{}",
    ),
    ("Unsupported file format: {}", "不支持的文件格式：{}"),
    ("Unsupported file version: {}", "不支持的文件版本：{}"),
    ("Unsupported file version", "不支持的文件版本"),
    ("Section {}-{}", "剖视 {}-{}"),
    ("Detail {}", "详图 {}"),
    ("Reference {} {}", "参考 {} {}"),
    ("Linear pattern {} × {}", "线性阵列 {} × {}"),
    ("Body · {} · {}", "体 · {} · {}"),
    ("Face · {} · {}", "面 · {} · {}"),
    ("Edge · {} · {}", "边 · {} · {}"),
    ("Valid  {}", "✓ 有效  {}"),
    ("Volume      {} {}³", "体积      {} {}³"),
    ("Surface area {} {}²", "表面积    {} {}²"),
    ("Centroid X  {} {}", "质心 X    {} {}"),
    ("Centroid Y  {} {}", "质心 Y    {} {}"),
    ("Centroid Z  {} {}", "质心 Z    {} {}"),
    ("Project Name {}", "项目名 {}"),
    ("Drawing Number {}", "图号 {}"),
    ("Scale {}", "比例 {}"),
    ("Units {}", "单位 {}"),
    ("Date {}", "日期 {}"),
    ("Author {}", "作者 {}"),
    (
        "Symmetric: select two endpoint-bearing entities, then the mirror line; the nearest endpoints are used",
        "对称：先选两个含端点实体，最后选择镜像线；使用两实体最近端点",
    ),
    (
        "Point on Object: select a point-bearing entity, then a line, circle, or arc",
        "点在线上：先选含点实体，再选直线、圆或圆弧",
    ),
    ("Could not write {}: {}", "无法写入 {}：{}"),
    ("Could not read {}: {}", "无法读取 {}：{}"),
    ("Could not create {}: {}", "无法创建 {}：{}"),
    ("Could not serialize body {}: {}", "无法序列化实体 {}：{}"),
    ("Could not encode project file: {}", "无法编码工程文件：{}"),
    (
        "Project file JSON is corrupt: {}",
        "工程文件 JSON 已损坏：{}",
    ),
    (
        "Body {} has invalid BREP data: {}",
        "实体 {} 的 BREP 编码无效：{}",
    ),
    ("Could not restore body {}: {}", "无法恢复实体 {}：{}"),
    (
        "Reference image data is invalid: {}",
        "参考图像编码无效：{}",
    ),
    ("HOME is not set", "HOME 未设置"),
    ("Depth {}", "深度 {}"),
    ("Linear Pattern", "线性阵列"),
    ("+ Save View", "+ 保存视图"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_prefix_mapping() {
        assert_eq!(lang_from_locale_str("zh-Hans-CN"), Some(Lang::ZhCn));
        assert_eq!(lang_from_locale_str("zh"), Some(Lang::ZhCn));
        assert_eq!(lang_from_locale_str("en-US"), Some(Lang::En));
        assert_eq!(lang_from_locale_str("fr-FR"), None);
        assert_eq!(lang_from_locale_str("fr-FR").unwrap_or(Lang::En), Lang::En);
    }

    #[test]
    fn chinese_lookup_and_missing_fallback() {
        assert_eq!(translate(Lang::ZhCn, "Extrude"), "拉伸");
        assert_eq!(
            translate(Lang::ZhCn, "Deliberately Missing"),
            "Deliberately Missing"
        );
    }

    #[test]
    fn one_argument_template_replacement() {
        assert_eq!(replace_one("Page {}", "2"), "Page 2");
        assert_eq!(tr1("Page {}", "2"), "Page 2");
    }
}
