//! Dependency-light interchange writers and the minimal DXF R12 sketch codec.

use std::{
    f64::consts::{PI, TAU},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use glam::DVec2;
use occt::{OcctMesh, Shape};
use serde_json::json;

use crate::sketch::{Sketch, SketchEntity, SketchItem, arc_center_radius};

pub(crate) fn meshes(shapes: &[&Shape]) -> Result<Vec<OcctMesh>, String> {
    shapes
        .iter()
        .map(|shape| shape.mesh(0.1).map_err(|error| error.to_string()))
        .collect()
}

pub(crate) fn write_obj(path: &Path, meshes: &[OcctMesh]) -> Result<(), String> {
    let mut output = String::from("# Free3D OBJ\n");
    let mut offset = 1_u32;
    for mesh in meshes {
        for point in &mesh.positions {
            output.push_str(&format!("v {} {} {}\n", point.x, point.y, point.z));
        }
        for normal in &mesh.normals {
            output.push_str(&format!("vn {} {} {}\n", normal.x, normal.y, normal.z));
        }
        for triangle in mesh.indices.chunks_exact(3) {
            let a = triangle[0] + offset;
            let b = triangle[1] + offset;
            let c = triangle[2] + offset;
            output.push_str(&format!("f {a}//{a} {b}//{b} {c}//{c}\n"));
        }
        offset += mesh.positions.len() as u32;
    }
    std::fs::write(path, output).map_err(|error| error.to_string())
}

fn mesh_binary(meshes: &[OcctMesh]) -> (Vec<u8>, Vec<(usize, usize, usize, usize)>) {
    let mut bytes = Vec::new();
    let mut views = Vec::new();
    for mesh in meshes {
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let position_start = bytes.len();
        for point in &mesh.positions {
            for value in [point.x as f32, point.y as f32, point.z as f32] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let position_len = bytes.len() - position_start;
        let index_start = bytes.len();
        for index in &mesh.indices {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        let index_len = bytes.len() - index_start;
        views.push((position_start, position_len, index_start, index_len));
    }
    (bytes, views)
}

fn gltf_json(
    meshes: &[OcctMesh],
    bytes: &[u8],
    views: &[(usize, usize, usize, usize)],
    uri: Option<String>,
) -> serde_json::Value {
    let mut buffer_views = Vec::new();
    let mut accessors = Vec::new();
    let mut primitives = Vec::new();
    for (index, (mesh, &(p0, plen, i0, ilen))) in meshes.iter().zip(views).enumerate() {
        buffer_views.push(json!({"buffer":0,"byteOffset":p0,"byteLength":plen,"target":34962}));
        buffer_views.push(json!({"buffer":0,"byteOffset":i0,"byteLength":ilen,"target":34963}));
        let (min, max) = mesh.positions.iter().fold(
            ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
            |(mut min, mut max), p| {
                for (slot, value) in [p.x, p.y, p.z].into_iter().enumerate() {
                    min[slot] = min[slot].min(value);
                    max[slot] = max[slot].max(value);
                }
                (min, max)
            },
        );
        accessors.push(json!({"bufferView":index*2,"componentType":5126,"count":mesh.positions.len(),"type":"VEC3","min":min,"max":max}));
        accessors.push(json!({"bufferView":index*2+1,"componentType":5125,"count":mesh.indices.len(),"type":"SCALAR"}));
        primitives.push(json!({"attributes":{"POSITION":index*2},"indices":index*2+1,"mode":4}));
    }
    let mut buffer = json!({"byteLength":bytes.len()});
    if let Some(uri) = uri {
        buffer["uri"] = json!(uri);
    }
    json!({"asset":{"version":"2.0","generator":"Free3D"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],"meshes":[{"primitives":primitives}],"buffers":[buffer],"bufferViews":buffer_views,"accessors":accessors})
}

pub(crate) fn write_gltf(path: &Path, meshes: &[OcctMesh], binary: bool) -> Result<(), String> {
    let (mut bytes, views) = mesh_binary(meshes);
    if binary {
        let mut json_bytes = serde_json::to_vec(&gltf_json(meshes, &bytes, &views, None))
            .map_err(|e| e.to_string())?;
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let total = 12 + 8 + json_bytes.len() + 8 + bytes.len();
        let mut glb = Vec::with_capacity(total);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bytes);
        std::fs::write(path, glb).map_err(|e| e.to_string())
    } else {
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            BASE64.encode(&bytes)
        );
        let json = serde_json::to_vec_pretty(&gltf_json(meshes, &bytes, &views, Some(uri)))
            .map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}

pub(crate) fn write_3mf(path: &Path, meshes: &[OcctMesh]) -> Result<(), String> {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02"><resources>"#,
    );
    for (object, mesh) in meshes.iter().enumerate() {
        xml.push_str(&format!(
            "<object id=\"{}\" type=\"model\"><mesh><vertices>",
            object + 1
        ));
        for p in &mesh.positions {
            xml.push_str(&format!(
                "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>",
                p.x, p.y, p.z
            ));
        }
        xml.push_str("</vertices><triangles>");
        for t in mesh.indices.chunks_exact(3) {
            xml.push_str(&format!(
                "<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                t[0], t[1], t[2]
            ));
        }
        xml.push_str("</triangles></mesh></object>");
    }
    xml.push_str("</resources><build>");
    for object in 0..meshes.len() {
        xml.push_str(&format!("<item objectid=\"{}\"/>", object + 1));
    }
    xml.push_str("</build></model>");
    let files: [(&str, &[u8]); 3] = [
        ("[Content_Types].xml", br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/></Types>"#),
        ("_rels/.rels", br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/></Relationships>"#),
        ("3D/3dmodel.model", xml.as_bytes()),
    ];
    write_stored_zip(path, &files)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn write_stored_zip(path: &Path, files: &[(&str, &[u8])]) -> Result<(), String> {
    let mut output = Vec::new();
    let mut directory = Vec::new();
    for &(name, data) in files {
        let offset = output.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name = name.as_bytes();
        output.extend_from_slice(&0x0403_4b50_u32.to_le_bytes());
        output.extend_from_slice(&20_u16.to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&size.to_le_bytes());
        output.extend_from_slice(&size.to_le_bytes());
        output.extend_from_slice(&(name.len() as u16).to_le_bytes());
        output.extend_from_slice(&0_u16.to_le_bytes());
        output.extend_from_slice(name);
        output.extend_from_slice(data);
        directory.extend_from_slice(&0x0201_4b50_u32.to_le_bytes());
        directory.extend_from_slice(&20_u16.to_le_bytes());
        directory.extend_from_slice(&20_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&crc.to_le_bytes());
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&size.to_le_bytes());
        directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u16.to_le_bytes());
        directory.extend_from_slice(&0_u32.to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name);
    }
    let directory_offset = output.len() as u32;
    let directory_size = directory.len() as u32;
    output.extend_from_slice(&directory);
    output.extend_from_slice(&0x0605_4b50_u32.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&(files.len() as u16).to_le_bytes());
    output.extend_from_slice(&(files.len() as u16).to_le_bytes());
    output.extend_from_slice(&directory_size.to_le_bytes());
    output.extend_from_slice(&directory_offset.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
    std::fs::write(path, output).map_err(|e| e.to_string())
}

fn pair(out: &mut String, code: i32, value: impl std::fmt::Display) {
    out.push_str(&format!("{code}\n{value}\n"));
}

pub(crate) fn write_dxf(path: &Path, sketches: &[&Sketch]) -> Result<(), String> {
    let mut out = String::new();
    pair(&mut out, 0, "SECTION");
    pair(&mut out, 2, "ENTITIES");
    for sketch in sketches {
        for item in &sketch.entities {
            match &item.geo {
                SketchEntity::Line { a, b } => {
                    pair(&mut out, 0, "LINE");
                    pair(&mut out, 8, "0");
                    pair(&mut out, 10, a.x);
                    pair(&mut out, 20, a.y);
                    pair(&mut out, 11, b.x);
                    pair(&mut out, 21, b.y);
                }
                SketchEntity::Circle { center, radius } => {
                    pair(&mut out, 0, "CIRCLE");
                    pair(&mut out, 8, "0");
                    pair(&mut out, 10, center.x);
                    pair(&mut out, 20, center.y);
                    pair(&mut out, 40, radius);
                }
                SketchEntity::Arc { start, mid, end } => {
                    if let Some((center, radius)) = arc_center_radius(*start, *mid, *end) {
                        let angle = |p: DVec2| {
                            (p.y - center.y)
                                .atan2(p.x - center.x)
                                .to_degrees()
                                .rem_euclid(360.0)
                        };
                        pair(&mut out, 0, "ARC");
                        pair(&mut out, 8, "0");
                        pair(&mut out, 10, center.x);
                        pair(&mut out, 20, center.y);
                        pair(&mut out, 40, radius);
                        pair(&mut out, 50, angle(*start));
                        pair(&mut out, 51, angle(*end));
                    }
                }
                SketchEntity::Ellipse {
                    center,
                    major,
                    minor_ratio,
                } => {
                    pair(&mut out, 0, "ELLIPSE");
                    pair(&mut out, 10, center.x);
                    pair(&mut out, 20, center.y);
                    pair(&mut out, 11, major.x);
                    pair(&mut out, 21, major.y);
                    pair(&mut out, 40, minor_ratio);
                    pair(&mut out, 41, 0);
                    pair(&mut out, 42, TAU);
                }
                SketchEntity::Spline { points } => write_polyline(&mut out, points, false),
                SketchEntity::CvSpline { control, degree } => {
                    let points = crate::sketch::sample_cv_spline(control, *degree, sketch.plane);
                    write_polyline(&mut out, &points, false);
                }
                SketchEntity::EllipseArc {
                    center,
                    major,
                    minor_ratio,
                    start_angle,
                    end_angle,
                } => {
                    let points: Vec<DVec2> = (0..=32)
                        .map(|i| {
                            let t = start_angle + (end_angle - start_angle) * i as f64 / 32.0;
                            let minor = DVec2::new(-major.y, major.x) * minor_ratio;
                            *center + *major * t.cos() + minor * t.sin()
                        })
                        .collect();
                    write_polyline(&mut out, &points, false);
                }
                SketchEntity::Point { at } => {
                    pair(&mut out, 0, "POINT");
                    pair(&mut out, 10, at.x);
                    pair(&mut out, 20, at.y);
                }
            }
        }
    }
    pair(&mut out, 0, "ENDSEC");
    pair(&mut out, 0, "EOF");
    std::fs::write(path, out).map_err(|e| e.to_string())
}

fn write_polyline(out: &mut String, points: &[DVec2], closed: bool) {
    pair(out, 0, "POLYLINE");
    pair(out, 8, "0");
    pair(out, 66, 1);
    pair(out, 70, if closed { 1 } else { 0 });
    for p in points {
        pair(out, 0, "VERTEX");
        pair(out, 10, p.x);
        pair(out, 20, p.y);
    }
    pair(out, 0, "SEQEND");
}

pub(crate) fn read_dxf(path: &Path) -> Result<Vec<SketchItem>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let lines: Vec<_> = text.lines().collect();
    if lines.len() % 2 != 0 {
        return Err("DXF has an unmatched group code".into());
    }
    let pairs: Vec<(i32, &str)> = lines
        .chunks_exact(2)
        .filter_map(|p| p[0].trim().parse().ok().map(|c| (c, p[1].trim())))
        .collect();
    let mut result = Vec::new();
    let mut i = 0;
    let mut in_entities = false;
    while i < pairs.len() {
        if pairs[i] == (0, "SECTION") && pairs.get(i + 1) == Some(&(2, "ENTITIES")) {
            in_entities = true;
            i += 2;
            continue;
        }
        if pairs[i] == (0, "ENDSEC") {
            in_entities = false;
        }
        if !in_entities || pairs[i].0 != 0 {
            i += 1;
            continue;
        }
        let kind = pairs[i].1;
        let start = i + 1;
        i = start;
        while i < pairs.len() && pairs[i].0 != 0 {
            i += 1;
        }
        let fields = &pairs[start..i];
        let value = |code: i32| {
            fields
                .iter()
                .find(|p| p.0 == code)
                .and_then(|p| p.1.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        if matches!(kind, "LWPOLYLINE" | "SPLINE") {
            let mut points = Vec::new();
            let mut x = None;
            for &(code, text) in fields {
                if code == 10 {
                    x = text.parse().ok();
                } else if code == 20
                    && let (Some(x), Ok(y)) = (x.take(), text.parse())
                {
                    points.push(DVec2::new(x, y));
                }
            }
            let closed = fields
                .iter()
                .find(|p| p.0 == 70)
                .and_then(|p| p.1.parse::<i32>().ok())
                .is_some_and(|flags| flags & 1 != 0);
            push_polyline_lines(&mut result, &points, closed);
            continue;
        }
        if kind == "POLYLINE" {
            let closed = fields
                .iter()
                .find(|p| p.0 == 70)
                .and_then(|p| p.1.parse::<i32>().ok())
                .is_some_and(|flags| flags & 1 != 0);
            let mut points = Vec::new();
            while i < pairs.len() && pairs[i] != (0, "SEQEND") {
                if pairs[i] == (0, "VERTEX") {
                    let begin = i + 1;
                    i = begin;
                    while i < pairs.len() && pairs[i].0 != 0 {
                        i += 1;
                    }
                    let vertex = &pairs[begin..i];
                    let get = |code| {
                        vertex
                            .iter()
                            .find(|p| p.0 == code)
                            .and_then(|p| p.1.parse().ok())
                            .unwrap_or(0.0)
                    };
                    points.push(DVec2::new(get(10), get(20)));
                } else {
                    i += 1;
                }
            }
            push_polyline_lines(&mut result, &points, closed);
            continue;
        }
        let entity = match kind {
            "LINE" => Some(SketchEntity::Line {
                a: DVec2::new(value(10), value(20)),
                b: DVec2::new(value(11), value(21)),
            }),
            "CIRCLE" => Some(SketchEntity::Circle {
                center: DVec2::new(value(10), value(20)),
                radius: value(40).abs(),
            }),
            "ARC" => {
                let c = DVec2::new(value(10), value(20));
                let r = value(40).abs();
                let a = value(50) * PI / 180.0;
                let b = value(51) * PI / 180.0;
                let sweep = (b - a).rem_euclid(TAU);
                Some(SketchEntity::Arc {
                    start: c + DVec2::new(a.cos(), a.sin()) * r,
                    mid: c + DVec2::new((a + sweep / 2.0).cos(), (a + sweep / 2.0).sin()) * r,
                    end: c + DVec2::new((a + sweep).cos(), (a + sweep).sin()) * r,
                })
            }
            "ELLIPSE" => Some(SketchEntity::Ellipse {
                center: DVec2::new(value(10), value(20)),
                major: DVec2::new(value(11), value(21)),
                minor_ratio: value(40),
            }),
            "POINT" => Some(SketchEntity::Point {
                at: DVec2::new(value(10), value(20)),
            }),
            _ => None,
        };
        if let Some(e) = entity {
            result.push(SketchItem::regular(e));
        }
    }
    Ok(result)
}

fn push_polyline_lines(result: &mut Vec<SketchItem>, points: &[DVec2], closed: bool) {
    for pair in points.windows(2) {
        result.push(SketchItem::regular(SketchEntity::Line {
            a: pair[0],
            b: pair[1],
        }));
    }
    if closed && points.len() > 2 {
        result.push(SketchItem::regular(SketchEntity::Line {
            a: *points.last().unwrap(),
            b: points[0],
        }));
    }
}
