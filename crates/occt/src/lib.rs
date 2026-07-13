//! Safe, application-oriented OpenCASCADE bindings for Ductile.

use std::{
    error::Error,
    fmt,
    ops::Range,
    path::Path,
    sync::{Mutex, PoisonError},
};

use cxx::UniquePtr;
use glam::DVec3;
use occt_bridge::ffi::{self, Point3, ShapeHandle, SurfaceKindRaw};

/// An error raised by OpenCASCADE or rejected by the bridge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OcctError {
    message: String,
}

impl OcctError {
    /// Returns the OCCT dynamic exception type and diagnostic message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for OcctError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for OcctError {}

impl From<cxx::Exception> for OcctError {
    fn from(error: cxx::Exception) -> Self {
        Self {
            message: error.what().to_owned(),
        }
    }
}

/// The analytic or freeform kind of an OCCT face surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceKind {
    /// A planar surface.
    Plane,
    /// A cylindrical surface.
    Cylinder,
    /// A spherical surface.
    Sphere,
    /// A conical surface.
    Cone,
    /// A toroidal surface.
    Torus,
    /// A Bezier surface.
    Bezier,
    /// A B-spline surface.
    BSpline,
    /// Any other OCCT surface type.
    Other,
}

/// A ray intersection attributed to the explorer-order face index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// Zero-based face index in `TopExp_Explorer` order.
    pub face_index: usize,
    /// Signed parameter along the normalized ray direction.
    pub t: f64,
    /// World-space intersection point.
    pub point: DVec3,
}

/// Visible and hidden projected polylines returned by hidden-line removal.
pub type HlrPolylines = (Vec<Vec<DVec3>>, Vec<Vec<DVec3>>);
/// Visible, hidden, and cut-outline polylines from a clipped section HLR.
pub type SectionHlrPolylines = (Vec<Vec<DVec3>>, Vec<Vec<DVec3>>, Vec<Vec<DVec3>>);

/// A complete shape mesh with triangle ranges parallel to face iteration order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcctMesh {
    /// Face-local positions concatenated in explorer order.
    pub positions: Vec<DVec3>,
    /// Unit vertex normals parallel to `positions`.
    pub normals: Vec<DVec3>,
    /// Triangle indices into `positions`.
    pub indices: Vec<u32>,
    /// Index-buffer ranges parallel to shape faces.
    pub face_ranges: Vec<Range<u32>>,
}

/// Volume, surface area, centroid, and centroidal inertia tensor of a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProperties {
    pub volume: f64,
    pub area: f64,
    pub center_of_mass: DVec3,
    /// Row-major symmetric 3×3 inertia tensor about `center_of_mass`.
    pub inertia_matrix: [f64; 9],
}

/// Computes raw unit-density mass properties using bridge-shaped arrays.
pub fn shape_volume_properties(shape: &Shape) -> Result<(f64, f64, [f64; 3], [f64; 9]), OcctError> {
    let properties = shape.volume_properties()?;
    Ok((
        properties.volume,
        properties.area,
        properties.center_of_mass.to_array(),
        properties.inertia_matrix,
    ))
}

/// Returns OCCT validity status names; an empty list means valid.
pub fn shape_check(shape: &Shape) -> Result<Vec<String>, OcctError> {
    shape.check()
}

/// An owning handle to an OCCT `TopoDS_Shape`.
pub struct Shape {
    inner: UniquePtr<ShapeHandle>,
}

/// An owning OCCT edge.
pub struct Edge(Shape);

impl Edge {
    /// Builds a finite straight edge.
    pub fn segment(start: DVec3, end: DVec3) -> Result<Self, OcctError> {
        Ok(Self(Shape::from_ffi(ffi::make_segment(
            to_point(start),
            to_point(end),
        )?)?))
    }

    /// Builds a full circular edge.
    pub fn circle(center: DVec3, normal: DVec3, radius: f64) -> Result<Self, OcctError> {
        Ok(Self(Shape::from_ffi(ffi::make_circle(
            to_point(center),
            to_point(normal),
            radius,
        )?)?))
    }

    /// Builds an arc passing through three ordered points.
    pub fn arc(start: DVec3, middle: DVec3, end: DVec3) -> Result<Self, OcctError> {
        Ok(Self(Shape::from_ffi(ffi::make_three_point_arc(
            to_point(start),
            to_point(middle),
            to_point(end),
        )?)?))
    }

    /// Builds an arc from a start point and tangent direction to an end point.
    pub fn tangent_arc(start: DVec3, tangent: DVec3, end: DVec3) -> Result<Self, OcctError> {
        Ok(Self(Shape::from_ffi(ffi::make_tangent_arc(
            to_point(start),
            to_point(tangent),
            to_point(end),
        )?)?))
    }

    /// Builds a full ellipse whose major direction lies in the ellipse plane.
    pub fn ellipse(
        center: DVec3,
        normal: DVec3,
        major_direction: DVec3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<Self, OcctError> {
        Ok(Self(Shape::from_ffi(ffi::make_ellipse(
            to_point(center),
            to_point(normal),
            to_point(major_direction),
            major_radius,
            minor_radius,
        )?)?))
    }

    /// Builds a counter-clockwise elliptical arc using ellipse-local angles.
    pub fn ellipse_arc(
        center: DVec3,
        normal: DVec3,
        major_direction: DVec3,
        major_radius: f64,
        minor_radius: f64,
        start_angle: f64,
        end_angle: f64,
    ) -> Result<Self, OcctError> {
        Ok(Self(Shape::from_ffi(ffi::make_ellipse_arc(
            to_point(center),
            to_point(normal),
            to_point(major_direction),
            major_radius,
            minor_radius,
            start_angle,
            end_angle,
        )?)?))
    }

    /// Interpolates a B-spline through the supplied points.
    pub fn spline_from_points(points: &[DVec3]) -> Result<Self, OcctError> {
        let points: Vec<_> = points.iter().copied().map(to_point).collect();
        Ok(Self(Shape::from_ffi(ffi::make_spline(&points)?)?))
    }

    /// Builds a clamped uniform B-spline whose control points are `poles`.
    pub fn bspline_from_poles(poles: &[DVec3], degree: u8) -> Result<Self, OcctError> {
        let poles: Vec<_> = poles.iter().copied().map(to_point).collect();
        Ok(Self(Shape::from_ffi(ffi::make_bspline_poles(
            &poles, degree,
        )?)?))
    }

    /// Borrows the edge as a general shape.
    pub fn as_shape(&self) -> &Shape {
        &self.0
    }
}

/// An owning ordered OCCT wire.
pub struct Wire(Shape);

impl Wire {
    /// Builds a connected wire from edges in traversal order.
    pub fn from_edges(edges: Vec<Edge>) -> Result<Self, OcctError> {
        let mut wire = ffi::make_wire()?;
        for edge in &edges {
            ffi::wire_add_edge(wire.pin_mut(), edge.0.as_ref())?;
        }
        Ok(Self(Shape::from_ffi(ffi::wire_build(wire.pin_mut())?)?))
    }

    /// Returns the wire as a general shape.
    pub fn into_shape(self) -> Shape {
        self.0
    }
}

/// An owning planar OCCT face.
pub struct Face(Shape);

impl Face {
    /// Builds a face from one closed outer wire.
    pub fn from_wire(outer: &Wire) -> Result<Self, OcctError> {
        Self::from_wire_with_holes(outer, &[])
    }

    /// Builds a face from a closed outer wire and closed hole wires.
    pub fn from_wire_with_holes(outer: &Wire, holes: &[Wire]) -> Result<Self, OcctError> {
        let mut face = ffi::make_face(outer.0.as_ref())?;
        for hole in holes {
            ffi::face_add_hole(face.pin_mut(), hole.0.as_ref())?;
        }
        Ok(Self(Shape::from_ffi(ffi::face_build(face.pin_mut())?)?))
    }

    /// Returns the face as a general shape.
    pub fn into_shape(self) -> Shape {
        self.0
    }

    /// Borrows the face as a general shape.
    pub fn as_shape(&self) -> &Shape {
        &self.0
    }
}

/// Serializes OCCT data exchange: the STEP/IGES translators mutate shared,
/// unsynchronized process globals (`Interface_Static` tables, one-shot
/// controller initialization), so concurrent calls crash.
static DATA_EXCHANGE_LOCK: Mutex<()> = Mutex::new(());

fn data_exchange_guard() -> std::sync::MutexGuard<'static, ()> {
    DATA_EXCHANGE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shape {
    /// Computes OCCT hidden-line removal and samples projected visible and hidden edges.
    pub fn hlr(&self, view_dir: DVec3, deflection: f64) -> Result<HlrPolylines, OcctError> {
        if !view_dir.is_finite() || view_dir.length_squared() <= f64::EPSILON {
            return Err(OcctError {
                message: "HLR view direction must be non-zero".to_owned(),
            });
        }
        let result = ffi::shape_hlr(self.as_ref(), to_point(view_dir.normalize()))?;
        let visible = Self::from_ffi(ffi::hlr_visible(result.as_ref().expect("HLR handle"))?)?;
        let hidden = Self::from_ffi(ffi::hlr_hidden(result.as_ref().expect("HLR handle"))?)?;
        fn sample(shape: &Shape, deflection: f64) -> Result<Vec<Vec<DVec3>>, OcctError> {
            (0..shape.edge_count()?)
                .map(|index| shape.edge_polyline(index, deflection))
                .filter(|points| points.as_ref().map_or(true, |points| points.len() >= 2))
                .collect()
        }
        Ok((sample(&visible, deflection)?, sample(&hidden, deflection)?))
    }

    /// Clips this shape by the positive side of a plane and performs HLR.
    pub fn section_hlr(
        &self,
        plane_origin: DVec3,
        plane_normal: DVec3,
        view_dir: DVec3,
        deflection: f64,
    ) -> Result<SectionHlrPolylines, OcctError> {
        if !plane_origin.is_finite()
            || !plane_normal.is_finite()
            || plane_normal.length_squared() <= f64::EPSILON
            || !view_dir.is_finite()
            || view_dir.length_squared() <= f64::EPSILON
        {
            return Err(OcctError {
                message: "section plane and view direction must be finite and non-zero".to_owned(),
            });
        }
        let result = ffi::shape_section_hlr(
            self.as_ref(),
            to_point(plane_origin),
            to_point(plane_normal.normalize()),
            to_point(view_dir.normalize()),
        )?;
        let result = result.as_ref().expect("section HLR handle");
        let visible = Self::from_ffi(ffi::hlr_visible(result)?)?;
        let hidden = Self::from_ffi(ffi::hlr_hidden(result)?)?;
        let section = Self::from_ffi(ffi::hlr_section(result)?)?;
        fn sample(shape: &Shape, deflection: f64) -> Result<Vec<Vec<DVec3>>, OcctError> {
            (0..shape.edge_count()?)
                .map(|index| shape.edge_polyline(index, deflection))
                .filter(|points| points.as_ref().map_or(true, |points| points.len() >= 2))
                .collect()
        }
        Ok((
            sample(&visible, deflection)?,
            sample(&hidden, deflection)?,
            sample(&section, deflection)?,
        ))
    }

    /// Constructs an axis-aligned cube from the origin.
    pub fn cube(size: f64) -> Result<Self, OcctError> {
        Self::box_from_corners(DVec3::ZERO, DVec3::splat(size))
    }
    /// Constructs an axis-aligned box from two opposite corners.
    pub fn box_from_corners(corner_1: DVec3, corner_2: DVec3) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_box(to_point(corner_1), to_point(corner_2))?)
    }

    /// Constructs a cylinder from an origin, radius, axis, and height.
    pub fn cylinder(
        origin: DVec3,
        radius: f64,
        axis: DVec3,
        height: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_cylinder(
            to_point(origin),
            radius,
            to_point(axis),
            height,
        )?)
    }

    /// Constructs a sphere at `center`.
    pub fn sphere(center: DVec3, radius: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_sphere(to_point(center), radius)?)
    }

    /// Constructs an axis-aligned ellipsoid centered at `center`.
    pub fn ellipsoid(center: DVec3, radii: DVec3) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_ellipsoid(
            to_point(center),
            radii.x,
            radii.y,
            radii.z,
        )?)
    }

    /// Constructs a regular world-Z prism centered on its base.
    pub fn regular_prism(
        center: DVec3,
        radius: f64,
        sides: u32,
        height: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_regular_prism(
            to_point(center),
            radius,
            sides,
            height,
        )?)
    }

    /// Constructs a world-Z wedge with the requested top X length.
    pub fn wedge(origin: DVec3, dx: f64, dy: f64, dz: f64, top_dx: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_wedge(to_point(origin), dx, dy, dz, top_dx)?)
    }

    /// Constructs a Z-axis cone with its base at `origin` and a point apex.
    pub fn cone(origin: DVec3, bottom_radius: f64, height: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_cone(to_point(origin), bottom_radius, height)?)
    }

    /// Constructs an axis-aligned conical frustum.
    pub fn cone_axis(
        origin: DVec3,
        bottom_radius: f64,
        top_radius: f64,
        axis: DVec3,
        height: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_cone_axis(
            to_point(origin),
            bottom_radius,
            top_radius,
            to_point(axis),
            height,
        )?)
    }

    /// Constructs a Z-axis torus centered at `center`.
    pub fn torus(center: DVec3, major_radius: f64, minor_radius: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_torus(
            to_point(center),
            major_radius,
            minor_radius,
        )?)
    }

    /// Builds a compound containing each supplied shape without modifying it.
    pub fn compound(shapes: Vec<Self>) -> Result<Self, OcctError> {
        let mut compound = ffi::make_compound()?;
        for shape in shapes {
            ffi::compound_add(compound.pin_mut(), shape.as_ref())?;
        }
        Self::from_ffi(compound)
    }

    /// Builds an exact cylindrical-surface helix as a single-edge wire.
    pub fn helix_wire(
        origin: DVec3,
        axis: DVec3,
        radius: f64,
        pitch: f64,
        turns: f64,
        left_handed: bool,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::make_helix_wire(
            to_point(origin),
            to_point(axis),
            radius,
            pitch,
            turns,
            left_handed,
        )?)
    }

    /// Fuses this shape with another shape.
    pub fn fuse(&self, other: &Self) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_fuse(self.as_ref(), other.as_ref())?)
    }

    /// Cuts another shape from this shape.
    pub fn cut(&self, other: &Self) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_cut(self.as_ref(), other.as_ref())?)
    }

    /// Computes the common volume of this shape and another shape.
    pub fn common(&self, other: &Self) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_common(self.as_ref(), other.as_ref())?)
    }

    /// Computes unit-density volume and surface properties in model units.
    pub fn volume_properties(&self) -> Result<MassProperties, OcctError> {
        let raw = ffi::shape_volume_properties(self.as_ref())?;
        let inertia_matrix: [f64; 9] =
            raw.inertia
                .try_into()
                .map_err(|values: Vec<f64>| OcctError {
                    message: format!(
                        "bridge returned {} inertia values, expected 9",
                        values.len()
                    ),
                })?;
        Ok(MassProperties {
            volume: raw.volume,
            area: raw.area,
            center_of_mass: from_point(raw.center),
            inertia_matrix,
        })
    }

    /// Returns OCCT validation status names; an empty list means valid.
    pub fn check(&self) -> Result<Vec<String>, OcctError> {
        Ok(ffi::shape_check(self.as_ref())?)
    }

    /// Extrudes one explorer-order face by a vector.
    pub fn extrude_face(&self, face_index: usize, vector: DVec3) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::prism_face(
            self.as_ref(),
            face_index,
            to_point(vector),
        )?)
    }

    /// Extrudes the first face of a standalone face-holding shape.
    pub fn prism_of_face_shape(&self, vector: DVec3) -> Result<Self, OcctError> {
        self.extrude_face(0, vector)
    }

    /// Extrudes the first wire into an open-sided shell.
    pub fn prism_of_wire_shape(&self, vector: DVec3) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::prism_wire(self.as_ref(), to_point(vector))?)
    }

    /// Revolves one explorer-order face around an axis by radians.
    pub fn revolve_face(
        &self,
        face_index: usize,
        axis_origin: DVec3,
        axis_direction: DVec3,
        angle_rad: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::revolve_face(
            self.as_ref(),
            face_index,
            to_point(axis_origin),
            to_point(axis_direction),
            angle_rad,
        )?)
    }

    /// Revolves the first wire into a surface shell.
    pub fn revolve_wire(
        &self,
        axis_origin: DVec3,
        axis_direction: DVec3,
        angle_rad: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::revolve_wire(
            self.as_ref(),
            to_point(axis_origin),
            to_point(axis_direction),
            angle_rad,
        )?)
    }

    /// Sweeps this profile shape along the first wire in `spine`.
    pub fn sweep_along(&self, spine: &Self) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::sweep_pipe(self.as_ref(), spine.as_ref())?)
    }

    /// Lofts a solid through the first wire of each supplied shape.
    pub fn loft(wires: Vec<Self>) -> Result<Self, OcctError> {
        let mut loft = ffi::make_loft()?;
        for wire in wires {
            ffi::loft_add_wire(loft.pin_mut(), wire.as_ref())?;
        }
        Self::from_ffi(ffi::loft_build(loft.pin_mut())?)
    }

    /// Fillets explorer-order edges.
    pub fn fillet_edges(&self, radius: f64, edge_indices: &[u32]) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::fillet_edges(self.as_ref(), radius, edge_indices)?)
    }

    /// Fillets edges with a linear radius law from each edge's start to end.
    pub fn variable_fillet_edges(
        &self,
        edge_indices: &[u32],
        start_radius: f64,
        end_radius: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::variable_fillet_edges(
            self.as_ref(),
            edge_indices,
            start_radius,
            end_radius,
        )?)
    }

    /// Chamfers explorer-order edges.
    pub fn chamfer_edges(&self, distance: f64, edge_indices: &[u32]) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::chamfer_edges(self.as_ref(), distance, edge_indices)?)
    }

    /// Removes faces and offsets the remainder to form a hollow solid.
    pub fn hollow(&self, face_indices: &[u32], thickness: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::hollow_shape(self.as_ref(), face_indices, thickness)?)
    }

    /// Applies a neutral-plane draft to explorer-order faces.
    pub fn draft_faces(
        &self,
        face_indices: &[u32],
        direction: DVec3,
        neutral_origin: DVec3,
        neutral_normal: DVec3,
        angle_rad: f64,
    ) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::draft_faces(
            self.as_ref(),
            face_indices,
            to_point(direction),
            to_point(neutral_origin),
            to_point(neutral_normal),
            angle_rad,
        )?)
    }

    /// Fills one closed explorer-order edge loop with a planar or fitted face.
    pub fn patch_face(&self, edge_indices: &[u32]) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::patch_face(self.as_ref(), edge_indices)?)
    }

    /// Sews the faces of all supplied shapes and promotes closed shells to solids.
    pub fn stitch(shapes: Vec<Self>, tolerance: f64) -> Result<Self, OcctError> {
        let compound = Self::compound(shapes)?;
        Self::from_ffi(ffi::stitch_shapes(compound.as_ref(), tolerance)?)
    }

    /// Offsets a surface on both sides and closes its boundary into a solid.
    pub fn thicken(&self, thickness: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::thicken_shape(self.as_ref(), thickness)?)
    }

    /// Removes explorer-order faces and heals the surrounding solid.
    pub fn delete_faces(&self, face_indices: &[u32]) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::delete_faces(self.as_ref(), face_indices)?)
    }

    /// Decodes OCCT binary BREP data.
    pub fn from_brep_data(data: &[u8]) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_from_brep_data(data)?)
    }

    /// Encodes this shape as OCCT binary BREP data.
    pub fn to_brep_data(&self) -> Result<Vec<u8>, OcctError> {
        Ok(ffi::shape_to_brep_data(self.as_ref())?)
    }

    /// Reads and transfers every root from a STEP file.
    pub fn from_step_file(path: &str) -> Result<Self, OcctError> {
        let _exchange = data_exchange_guard();
        Self::from_ffi(ffi::shape_from_step_file(path)?)
    }

    /// Reads and transfers every root from a STEP file.
    pub fn read_step(path: &Path) -> Result<Self, OcctError> {
        Self::from_step_file(path_string(path)?)
    }

    /// Reads and transfers every root from an IGES file.
    pub fn read_iges(path: &Path) -> Result<Self, OcctError> {
        let _exchange = data_exchange_guard();
        Self::from_ffi(ffi::shape_from_iges_file(path_string(path)?)?)
    }

    /// Reads an STL triangle mesh and sews its triangles into a shell or solid.
    pub fn read_stl(path: &Path) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_from_stl_file(path_string(path)?)?)
    }

    /// Writes one shape as STEP.
    pub fn to_step_file(&self, path: &str) -> Result<(), OcctError> {
        Self::write_step_refs([self], Path::new(path))
    }

    /// Writes multiple independent STEP roots.
    pub fn write_step_refs<'a>(
        shapes: impl IntoIterator<Item = &'a Shape>,
        path: &Path,
    ) -> Result<(), OcctError> {
        let _exchange = data_exchange_guard();
        let mut writer = ffi::make_step_writer()?;
        for shape in shapes {
            ffi::step_writer_add(writer.pin_mut(), shape.as_ref())?;
        }
        Ok(ffi::step_writer_write(
            writer.pin_mut(),
            path_string(path)?,
        )?)
    }

    /// Writes multiple independent shapes to one IGES file.
    pub fn write_iges_refs<'a>(
        shapes: impl IntoIterator<Item = &'a Shape>,
        path: &Path,
    ) -> Result<(), OcctError> {
        let _exchange = data_exchange_guard();
        let mut writer = ffi::make_iges_writer()?;
        for shape in shapes {
            ffi::iges_writer_add(writer.pin_mut(), shape.as_ref())?;
        }
        Ok(ffi::iges_writer_write(
            writer.pin_mut(),
            path_string(path)?,
        )?)
    }

    /// Meshes and writes one shape as STL.
    pub fn write_stl(&self, path: &Path, tolerance: f64) -> Result<(), OcctError> {
        Ok(ffi::shape_to_stl_file(
            self.as_ref(),
            path_string(path)?,
            tolerance,
        )?)
    }

    /// Returns whether the underlying OCCT handle contains a null shape.
    pub fn is_null(&self) -> bool {
        ffi::shape_is_null(self.as_ref())
    }

    /// Fallibly clones the OCCT shape handle.
    pub fn try_clone(&self) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_clone(self.as_ref())?)
    }

    /// Returns a translated copy.
    pub fn translated(&self, delta: DVec3) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_translated(
            self.as_ref(),
            delta.x,
            delta.y,
            delta.z,
        )?)
    }

    /// Returns a copy rotated around an axis by radians.
    pub fn rotated(&self, origin: DVec3, axis: DVec3, angle_rad: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_rotated(
            self.as_ref(),
            to_point(origin),
            to_point(axis),
            angle_rad,
        )?)
    }

    /// Returns a uniformly scaled copy.
    pub fn scaled(&self, pivot: DVec3, factor: f64) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_scaled(self.as_ref(), to_point(pivot), factor)?)
    }

    /// Returns a copy mirrored across a plane.
    pub fn mirrored(&self, plane_origin: DVec3, plane_normal: DVec3) -> Result<Self, OcctError> {
        Self::from_ffi(ffi::shape_mirrored(
            self.as_ref(),
            to_point(plane_origin),
            to_point(plane_normal),
        )?)
    }

    /// Returns the number of faces in `TopExp_Explorer` order.
    pub fn face_count(&self) -> Result<usize, OcctError> {
        Ok(ffi::face_count(self.as_ref())?)
    }

    /// Returns the number of edges in `TopExp_Explorer` order.
    pub fn edge_count(&self) -> Result<usize, OcctError> {
        Ok(ffi::edge_count(self.as_ref())?)
    }

    /// Returns the number of solids in `TopExp_Explorer` order.
    pub fn solid_count(&self) -> Result<usize, OcctError> {
        Ok(ffi::solid_count(self.as_ref())?)
    }

    /// Returns owning shape copies for all contained solids.
    pub fn solids(&self) -> Result<Vec<Self>, OcctError> {
        (0..self.solid_count()?)
            .map(|index| Self::from_ffi(ffi::solid_at(self.as_ref(), index)?))
            .collect()
    }

    /// Computes the surface center of mass for one face.
    pub fn face_center_of_mass(&self, index: usize) -> Result<DVec3, OcctError> {
        Ok(from_point(ffi::face_center_of_mass(self.as_ref(), index)?))
    }

    /// Computes the surface area of one face.
    pub fn face_area(&self, index: usize) -> Result<f64, OcctError> {
        Ok(ffi::face_area(self.as_ref(), index)?)
    }

    /// Computes the outward-oriented normal at a point projected onto one face.
    pub fn face_normal_at(&self, index: usize, point: DVec3) -> Result<DVec3, OcctError> {
        Ok(from_point(ffi::face_normal_at(
            self.as_ref(),
            index,
            to_point(point),
        )?))
    }

    /// Classifies one face's underlying surface.
    pub fn face_surface_kind(&self, index: usize) -> Result<SurfaceKind, OcctError> {
        Ok(match ffi::face_surface_kind(self.as_ref(), index)? {
            SurfaceKindRaw::Plane => SurfaceKind::Plane,
            SurfaceKindRaw::Cylinder => SurfaceKind::Cylinder,
            SurfaceKindRaw::Sphere => SurfaceKind::Sphere,
            SurfaceKindRaw::Cone => SurfaceKind::Cone,
            SurfaceKindRaw::Torus => SurfaceKind::Torus,
            SurfaceKindRaw::Bezier => SurfaceKind::Bezier,
            SurfaceKindRaw::BSpline => SurfaceKind::BSpline,
            SurfaceKindRaw::Other => SurfaceKind::Other,
            _ => SurfaceKind::Other,
        })
    }

    /// Returns the lower axis point, unit axis, radius, and trimmed face height.
    pub fn face_cylinder_data(&self, index: usize) -> Result<(DVec3, DVec3, f64, f64), OcctError> {
        let data = ffi::face_cylinder_data(self.as_ref(), index)?;
        Ok((
            from_point(data.origin),
            from_point(data.axis),
            data.radius,
            data.height,
        ))
    }

    /// Returns whether one face has OCCT's reversed orientation flag.
    pub fn face_is_reversed(&self, index: usize) -> Result<bool, OcctError> {
        Ok(ffi::face_is_reversed(self.as_ref(), index)?)
    }

    /// Returns whether a face contains an edge from the shape's explorer order.
    pub fn face_contains_edge(
        &self,
        face_index: usize,
        edge_index: usize,
    ) -> Result<bool, OcctError> {
        Ok(ffi::face_contains_edge(
            self.as_ref(),
            face_index,
            edge_index,
        )?)
    }

    /// Returns the first curve endpoint of an edge.
    pub fn edge_start_point(&self, index: usize) -> Result<DVec3, OcctError> {
        Ok(from_point(ffi::edge_start_point(self.as_ref(), index)?))
    }

    /// Returns the last curve endpoint of an edge.
    pub fn edge_end_point(&self, index: usize) -> Result<DVec3, OcctError> {
        Ok(from_point(ffi::edge_end_point(self.as_ref(), index)?))
    }

    /// Computes the exact curve length of one edge.
    pub fn edge_length(&self, index: usize) -> Result<f64, OcctError> {
        Ok(ffi::edge_length(self.as_ref(), index)?)
    }

    /// Samples one edge to the requested linear deflection.
    pub fn edge_polyline(&self, index: usize, deflection: f64) -> Result<Vec<DVec3>, OcctError> {
        Ok(ffi::edge_polyline(self.as_ref(), index, deflection)?
            .into_iter()
            .map(from_point)
            .collect())
    }

    /// Computes the axis-aligned bounds including OCCT's shape tolerance gap.
    pub fn aabb(&self) -> Result<(DVec3, DVec3), OcctError> {
        let bounds = ffi::shape_aabb(self.as_ref())?;
        Ok((from_point(bounds.min), from_point(bounds.max)))
    }

    /// Intersects an infinite line and returns face-attributed hits.
    pub fn ray_hits(&self, origin: DVec3, direction: DVec3) -> Result<Vec<RayHit>, OcctError> {
        Ok(
            ffi::shape_ray_hits(self.as_ref(), to_point(origin), to_point(direction))?
                .into_iter()
                .map(|hit| RayHit {
                    face_index: hit.face_index as usize,
                    t: hit.t,
                    point: from_point(hit.point),
                })
                .collect(),
        )
    }

    /// Triangulates all faces in one OCCT meshing operation.
    pub fn mesh(&self, tolerance: f64) -> Result<OcctMesh, OcctError> {
        let raw = ffi::mesh_shape(self.as_ref(), tolerance)?;
        let face_ranges = raw
            .face_starts
            .into_iter()
            .zip(raw.face_ends)
            .map(|(start, end)| start..end)
            .collect();
        Ok(OcctMesh {
            positions: raw.positions.into_iter().map(from_point).collect(),
            normals: raw.normals.into_iter().map(from_point).collect(),
            indices: raw.indices,
            face_ranges,
        })
    }

    fn as_ref(&self) -> &ShapeHandle {
        self.inner
            .as_ref()
            .expect("a safe Shape always owns a non-null C++ handle")
    }

    fn from_ffi(inner: UniquePtr<ShapeHandle>) -> Result<Self, OcctError> {
        if inner.is_null() {
            Err(OcctError {
                message: "bridge returned a null ShapeHandle".to_owned(),
            })
        } else {
            Ok(Self { inner })
        }
    }
}

impl Clone for Shape {
    fn clone(&self) -> Self {
        self.try_clone()
            .expect("cloning an existing TopoDS_Shape should not fail")
    }
}

impl fmt::Debug for Shape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Shape")
            .field("is_null", &self.is_null())
            .finish_non_exhaustive()
    }
}

fn to_point(value: DVec3) -> Point3 {
    Point3 {
        x: value.x,
        y: value.y,
        z: value.z,
    }
}

fn from_point(value: Point3) -> DVec3 {
    DVec3::new(value.x, value.y, value.z)
}

fn path_string(path: &Path) -> Result<&str, OcctError> {
    path.to_str().ok_or_else(|| OcctError {
        message: format!("path is not valid UTF-8: {}", path.display()),
    })
}
