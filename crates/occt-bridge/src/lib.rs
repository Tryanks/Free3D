//! Raw CXX bridge for the subset of OpenCASCADE used by Ductile.

#[cxx::bridge]
pub mod ffi {
    /// A three-dimensional point or vector crossing the bridge.
    #[derive(Clone, Copy, Debug)]
    struct Point3 {
        x: f64,
        y: f64,
        z: f64,
    }

    /// Raw axis-aligned bounds.
    #[derive(Clone, Copy, Debug)]
    struct Bounds {
        min: Point3,
        max: Point3,
    }

    /// Raw geometric mass properties in the shape's model units.
    #[derive(Clone, Debug)]
    struct MassPropertiesRaw {
        volume: f64,
        area: f64,
        center: Point3,
        inertia: Vec<f64>,
    }

    /// OCCT surface classification.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum SurfaceKindRaw {
        Plane,
        Cylinder,
        Sphere,
        Cone,
        Torus,
        Bezier,
        BSpline,
        Other,
    }

    /// One ray intersection in topology explorer face-index space.
    #[derive(Clone, Copy, Debug)]
    struct RayHitRaw {
        face_index: u32,
        t: f64,
        point: Point3,
    }
    /// Axis, trimmed height, and radius of a cylindrical face.
    #[derive(Clone, Copy, Debug)]
    struct CylinderDataRaw {
        origin: Point3,
        axis: Point3,
        radius: f64,
        height: f64,
    }

    /// Flat triangulation buffers with parallel face index ranges.
    #[derive(Debug)]
    struct MeshRaw {
        positions: Vec<Point3>,
        normals: Vec<Point3>,
        indices: Vec<u32>,
        face_starts: Vec<u32>,
        face_ends: Vec<u32>,
    }

    unsafe extern "C++" {
        include!("occt_bridge.h");

        type ShapeHandle;
        type LoftHandle;
        type WireHandle;
        type FaceHandle;
        type StepWriterHandle;
        type IgesWriterHandle;
        type HlrHandle;

        fn shape_clone(shape: &ShapeHandle) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_is_null(shape: &ShapeHandle) -> bool;
        fn shape_volume_properties(shape: &ShapeHandle) -> Result<MassPropertiesRaw>;
        fn shape_check(shape: &ShapeHandle) -> Result<Vec<String>>;
        fn shape_hlr(shape: &ShapeHandle, view_dir: Point3) -> Result<UniquePtr<HlrHandle>>;
        fn shape_section_hlr(
            shape: &ShapeHandle,
            plane_origin: Point3,
            plane_normal: Point3,
            view_dir: Point3,
        ) -> Result<UniquePtr<HlrHandle>>;
        fn hlr_visible(hlr: &HlrHandle) -> Result<UniquePtr<ShapeHandle>>;
        fn hlr_hidden(hlr: &HlrHandle) -> Result<UniquePtr<ShapeHandle>>;
        fn hlr_section(hlr: &HlrHandle) -> Result<UniquePtr<ShapeHandle>>;

        fn make_box(corner_1: Point3, corner_2: Point3) -> Result<UniquePtr<ShapeHandle>>;
        fn make_cylinder(
            origin: Point3,
            radius: f64,
            axis: Point3,
            height: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_sphere(center: Point3, radius: f64) -> Result<UniquePtr<ShapeHandle>>;
        fn make_ellipsoid(
            center: Point3,
            x_radius: f64,
            y_radius: f64,
            z_radius: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_regular_prism(
            center: Point3,
            radius: f64,
            sides: u32,
            height: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_wedge(
            origin: Point3,
            dx: f64,
            dy: f64,
            dz: f64,
            top_dx: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_cone(
            origin: Point3,
            bottom_radius: f64,
            height: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_cone_axis(
            origin: Point3,
            bottom_radius: f64,
            top_radius: f64,
            axis: Point3,
            height: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_torus(
            center: Point3,
            major_radius: f64,
            minor_radius: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_compound() -> Result<UniquePtr<ShapeHandle>>;
        fn compound_add(compound: Pin<&mut ShapeHandle>, child: &ShapeHandle) -> Result<()>;

        fn make_segment(start: Point3, end: Point3) -> Result<UniquePtr<ShapeHandle>>;
        fn make_circle(
            center: Point3,
            normal: Point3,
            radius: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_three_point_arc(
            start: Point3,
            middle: Point3,
            end: Point3,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_tangent_arc(
            start: Point3,
            tangent: Point3,
            end: Point3,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_ellipse(
            center: Point3,
            normal: Point3,
            major_direction: Point3,
            major_radius: f64,
            minor_radius: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_ellipse_arc(
            center: Point3,
            normal: Point3,
            major_direction: Point3,
            major_radius: f64,
            minor_radius: f64,
            start_angle: f64,
            end_angle: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_spline(points: &[Point3]) -> Result<UniquePtr<ShapeHandle>>;
        fn make_bspline_poles(poles: &[Point3], degree: u8) -> Result<UniquePtr<ShapeHandle>>;
        fn make_helix_wire(
            origin: Point3,
            axis: Point3,
            radius: f64,
            pitch: f64,
            turns: f64,
            left_handed: bool,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn make_wire() -> Result<UniquePtr<WireHandle>>;
        fn wire_add_edge(wire: Pin<&mut WireHandle>, edge: &ShapeHandle) -> Result<()>;
        fn wire_build(wire: Pin<&mut WireHandle>) -> Result<UniquePtr<ShapeHandle>>;
        fn make_face(outer: &ShapeHandle) -> Result<UniquePtr<FaceHandle>>;
        fn face_add_hole(face: Pin<&mut FaceHandle>, hole: &ShapeHandle) -> Result<()>;
        fn face_build(face: Pin<&mut FaceHandle>) -> Result<UniquePtr<ShapeHandle>>;

        fn shape_fuse(left: &ShapeHandle, right: &ShapeHandle) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_cut(left: &ShapeHandle, right: &ShapeHandle) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_common(left: &ShapeHandle, right: &ShapeHandle) -> Result<UniquePtr<ShapeHandle>>;
        fn prism_face(
            shape: &ShapeHandle,
            face_index: usize,
            vector: Point3,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn prism_wire(shape: &ShapeHandle, vector: Point3) -> Result<UniquePtr<ShapeHandle>>;
        fn revolve_face(
            shape: &ShapeHandle,
            face_index: usize,
            axis_origin: Point3,
            axis_direction: Point3,
            angle_rad: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn revolve_wire(
            shape: &ShapeHandle,
            axis_origin: Point3,
            axis_direction: Point3,
            angle_rad: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn sweep_pipe(profile: &ShapeHandle, spine: &ShapeHandle)
        -> Result<UniquePtr<ShapeHandle>>;
        fn make_loft() -> Result<UniquePtr<LoftHandle>>;
        fn loft_add_wire(loft: Pin<&mut LoftHandle>, wire: &ShapeHandle) -> Result<()>;
        fn loft_build(loft: Pin<&mut LoftHandle>) -> Result<UniquePtr<ShapeHandle>>;
        fn fillet_edges(
            shape: &ShapeHandle,
            radius: f64,
            edge_indices: &[u32],
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn variable_fillet_edges(
            shape: &ShapeHandle,
            edge_indices: &[u32],
            start_radius: f64,
            end_radius: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn chamfer_edges(
            shape: &ShapeHandle,
            distance: f64,
            edge_indices: &[u32],
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn hollow_shape(
            shape: &ShapeHandle,
            face_indices: &[u32],
            thickness: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn draft_faces(
            shape: &ShapeHandle,
            face_indices: &[u32],
            direction: Point3,
            neutral_origin: Point3,
            neutral_normal: Point3,
            angle_rad: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn patch_face(shape: &ShapeHandle, edge_indices: &[u32]) -> Result<UniquePtr<ShapeHandle>>;
        fn stitch_shapes(shapes: &ShapeHandle, tolerance: f64) -> Result<UniquePtr<ShapeHandle>>;
        fn thicken_shape(shape: &ShapeHandle, thickness: f64) -> Result<UniquePtr<ShapeHandle>>;
        fn delete_faces(
            shape: &ShapeHandle,
            face_indices: &[u32],
        ) -> Result<UniquePtr<ShapeHandle>>;

        fn shape_translated(
            shape: &ShapeHandle,
            dx: f64,
            dy: f64,
            dz: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_rotated(
            shape: &ShapeHandle,
            origin: Point3,
            axis: Point3,
            angle_rad: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_scaled(
            shape: &ShapeHandle,
            pivot: Point3,
            factor: f64,
        ) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_mirrored(
            shape: &ShapeHandle,
            plane_origin: Point3,
            plane_normal: Point3,
        ) -> Result<UniquePtr<ShapeHandle>>;

        fn face_count(shape: &ShapeHandle) -> Result<usize>;
        fn edge_count(shape: &ShapeHandle) -> Result<usize>;
        fn solid_count(shape: &ShapeHandle) -> Result<usize>;
        fn solid_at(shape: &ShapeHandle, index: usize) -> Result<UniquePtr<ShapeHandle>>;
        fn face_center_of_mass(shape: &ShapeHandle, index: usize) -> Result<Point3>;
        fn face_area(shape: &ShapeHandle, index: usize) -> Result<f64>;
        fn face_normal_at(shape: &ShapeHandle, index: usize, point: Point3) -> Result<Point3>;
        fn face_surface_kind(shape: &ShapeHandle, index: usize) -> Result<SurfaceKindRaw>;
        fn face_cylinder_data(shape: &ShapeHandle, index: usize) -> Result<CylinderDataRaw>;
        fn face_is_reversed(shape: &ShapeHandle, index: usize) -> Result<bool>;
        fn face_contains_edge(
            shape: &ShapeHandle,
            face_index: usize,
            edge_index: usize,
        ) -> Result<bool>;
        fn edge_start_point(shape: &ShapeHandle, index: usize) -> Result<Point3>;
        fn edge_end_point(shape: &ShapeHandle, index: usize) -> Result<Point3>;
        fn edge_length(shape: &ShapeHandle, index: usize) -> Result<f64>;
        fn edge_polyline(shape: &ShapeHandle, index: usize, deflection: f64)
        -> Result<Vec<Point3>>;
        fn shape_aabb(shape: &ShapeHandle) -> Result<Bounds>;
        fn shape_ray_hits(
            shape: &ShapeHandle,
            origin: Point3,
            direction: Point3,
        ) -> Result<Vec<RayHitRaw>>;
        fn mesh_shape(shape: &ShapeHandle, tolerance: f64) -> Result<MeshRaw>;

        fn shape_to_brep_data(shape: &ShapeHandle) -> Result<Vec<u8>>;
        fn shape_from_brep_data(data: &[u8]) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_from_step_file(path: &str) -> Result<UniquePtr<ShapeHandle>>;
        fn shape_from_iges_file(path: &str) -> Result<UniquePtr<ShapeHandle>>;
        fn make_step_writer() -> Result<UniquePtr<StepWriterHandle>>;
        fn step_writer_add(writer: Pin<&mut StepWriterHandle>, shape: &ShapeHandle) -> Result<()>;
        fn step_writer_write(writer: Pin<&mut StepWriterHandle>, path: &str) -> Result<()>;
        fn make_iges_writer() -> Result<UniquePtr<IgesWriterHandle>>;
        fn iges_writer_add(writer: Pin<&mut IgesWriterHandle>, shape: &ShapeHandle) -> Result<()>;
        fn iges_writer_write(writer: Pin<&mut IgesWriterHandle>, path: &str) -> Result<()>;
        fn shape_to_stl_file(shape: &ShapeHandle, path: &str, tolerance: f64) -> Result<()>;
        fn shape_from_stl_file(path: &str) -> Result<UniquePtr<ShapeHandle>>;
    }
}
