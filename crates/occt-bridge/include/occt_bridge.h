#pragma once

#include "rust/cxx.h"
class ShapeHandle;
class LoftHandle;
class WireHandle;
class FaceHandle;
class StepWriterHandle;
class IgesWriterHandle;
class HlrHandle;
#include "occt-bridge/src/lib.rs.h"

#include <TopoDS_Shape.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <STEPControl_Writer.hxx>
#include <IGESControl_Writer.hxx>
#include <memory>
#include <cstdint>

class ShapeHandle {
public:
  ShapeHandle();
  explicit ShapeHandle(const TopoDS_Shape &shape);

  TopoDS_Shape shape;
};

class LoftHandle {
public:
  LoftHandle();
  BRepOffsetAPI_ThruSections builder;
};

class WireHandle { public: BRepBuilderAPI_MakeWire builder; };
class FaceHandle {
public:
  explicit FaceHandle(const TopoDS_Wire &outer);
  BRepBuilderAPI_MakeFace builder;
};
class StepWriterHandle { public: STEPControl_Writer writer; };
class IgesWriterHandle { public: IGESControl_Writer writer; };
class HlrHandle {
public:
  HlrHandle(const TopoDS_Shape &visible, const TopoDS_Shape &hidden,
            const TopoDS_Shape &section = TopoDS_Shape());
  TopoDS_Shape visible;
  TopoDS_Shape hidden;
  TopoDS_Shape section;
};

std::unique_ptr<ShapeHandle> shape_clone(const ShapeHandle &shape);
bool shape_is_null(const ShapeHandle &shape) noexcept;
MassPropertiesRaw shape_volume_properties(const ShapeHandle &shape);
rust::Vec<rust::String> shape_check(const ShapeHandle &shape);
std::unique_ptr<HlrHandle> shape_hlr(const ShapeHandle &shape, Point3 view_dir);
std::unique_ptr<HlrHandle> shape_section_hlr(const ShapeHandle &shape,
                                             Point3 plane_origin,
                                             Point3 plane_normal,
                                             Point3 view_dir);
std::unique_ptr<ShapeHandle> hlr_visible(const HlrHandle &hlr);
std::unique_ptr<ShapeHandle> hlr_hidden(const HlrHandle &hlr);
std::unique_ptr<ShapeHandle> hlr_section(const HlrHandle &hlr);

std::unique_ptr<ShapeHandle> make_box(Point3 corner_1, Point3 corner_2);
std::unique_ptr<ShapeHandle> make_cylinder(Point3 origin, double radius,
                                           Point3 axis, double height);
std::unique_ptr<ShapeHandle> make_sphere(Point3 center, double radius);
std::unique_ptr<ShapeHandle> make_ellipsoid(Point3 center, double x_radius,
                                            double y_radius, double z_radius);
std::unique_ptr<ShapeHandle> make_regular_prism(Point3 center, double radius,
                                                std::uint32_t sides, double height);
std::unique_ptr<ShapeHandle> make_wedge(Point3 origin, double dx, double dy,
                                        double dz, double top_dx);
std::unique_ptr<ShapeHandle> make_cone(Point3 origin, double bottom_radius,
                                       double height);
std::unique_ptr<ShapeHandle> make_cone_axis(Point3 origin, double bottom_radius,
                                            double top_radius, Point3 axis,
                                            double height);
std::unique_ptr<ShapeHandle> make_torus(Point3 center, double major_radius,
                                        double minor_radius);
std::unique_ptr<ShapeHandle> make_compound();
void compound_add(ShapeHandle &compound, const ShapeHandle &child);
std::unique_ptr<ShapeHandle> make_segment(Point3 start, Point3 end);
std::unique_ptr<ShapeHandle> make_circle(Point3 center, Point3 normal, double radius);
std::unique_ptr<ShapeHandle> make_three_point_arc(Point3 start, Point3 middle, Point3 end);
std::unique_ptr<ShapeHandle> make_tangent_arc(Point3 start, Point3 tangent, Point3 end);
std::unique_ptr<ShapeHandle> make_ellipse(Point3 center, Point3 normal,
                                          Point3 major_direction,
                                          double major_radius, double minor_radius);
std::unique_ptr<ShapeHandle> make_ellipse_arc(Point3 center, Point3 normal,
                                              Point3 major_direction,
                                              double major_radius, double minor_radius,
                                              double start_angle, double end_angle);
std::unique_ptr<ShapeHandle> make_spline(rust::Slice<const Point3> points);
std::unique_ptr<ShapeHandle>
make_bspline_poles(rust::Slice<const Point3> poles, std::uint8_t degree);
std::unique_ptr<ShapeHandle> make_helix_wire(Point3 origin, Point3 axis,
                                             double radius, double pitch,
                                             double turns, bool left_handed);
std::unique_ptr<WireHandle> make_wire();
void wire_add_edge(WireHandle &wire, const ShapeHandle &edge);
std::unique_ptr<ShapeHandle> wire_build(WireHandle &wire);
std::unique_ptr<FaceHandle> make_face(const ShapeHandle &outer);
void face_add_hole(FaceHandle &face, const ShapeHandle &hole);
std::unique_ptr<ShapeHandle> face_build(FaceHandle &face);

std::unique_ptr<ShapeHandle> shape_fuse(const ShapeHandle &left,
                                        const ShapeHandle &right);
std::unique_ptr<ShapeHandle> shape_cut(const ShapeHandle &left,
                                       const ShapeHandle &right);
std::unique_ptr<ShapeHandle> shape_common(const ShapeHandle &left,
                                          const ShapeHandle &right);
std::unique_ptr<ShapeHandle> prism_face(const ShapeHandle &shape,
                                        std::size_t face_index, Point3 vector);
std::unique_ptr<ShapeHandle> prism_wire(const ShapeHandle &shape, Point3 vector);
std::unique_ptr<ShapeHandle> revolve_face(const ShapeHandle &shape,
                                          std::size_t face_index,
                                          Point3 axis_origin,
                                          Point3 axis_direction,
                                          double angle_rad);
std::unique_ptr<ShapeHandle> revolve_wire(const ShapeHandle &shape,
                                          Point3 axis_origin,
                                          Point3 axis_direction,
                                          double angle_rad);
std::unique_ptr<ShapeHandle> sweep_pipe(const ShapeHandle &profile,
                                        const ShapeHandle &spine);
std::unique_ptr<LoftHandle> make_loft();
void loft_add_wire(LoftHandle &loft, const ShapeHandle &wire);
std::unique_ptr<ShapeHandle> loft_build(LoftHandle &loft);
std::unique_ptr<ShapeHandle>
fillet_edges(const ShapeHandle &shape, double radius,
             rust::Slice<const std::uint32_t> edge_indices);
std::unique_ptr<ShapeHandle>
variable_fillet_edges(const ShapeHandle &shape,
                      rust::Slice<const std::uint32_t> edge_indices,
                      double start_radius, double end_radius);
std::unique_ptr<ShapeHandle>
chamfer_edges(const ShapeHandle &shape, double distance,
              rust::Slice<const std::uint32_t> edge_indices);
std::unique_ptr<ShapeHandle>
hollow_shape(const ShapeHandle &shape,
             rust::Slice<const std::uint32_t> face_indices, double thickness);
std::unique_ptr<ShapeHandle>
draft_faces(const ShapeHandle &shape,
            rust::Slice<const std::uint32_t> face_indices, Point3 direction,
            Point3 neutral_origin, Point3 neutral_normal, double angle_rad);
std::unique_ptr<ShapeHandle>
patch_face(const ShapeHandle &shape,
           rust::Slice<const std::uint32_t> edge_indices);
std::unique_ptr<ShapeHandle> stitch_shapes(const ShapeHandle &shapes,
                                           double tolerance);
std::unique_ptr<ShapeHandle> thicken_shape(const ShapeHandle &shape,
                                           double thickness);
std::unique_ptr<ShapeHandle>
delete_faces(const ShapeHandle &shape,
             rust::Slice<const std::uint32_t> face_indices);

std::unique_ptr<ShapeHandle> shape_translated(const ShapeHandle &shape,
                                              double dx, double dy, double dz);
std::unique_ptr<ShapeHandle> shape_rotated(const ShapeHandle &shape,
                                           Point3 origin, Point3 axis,
                                           double angle_rad);
std::unique_ptr<ShapeHandle> shape_scaled(const ShapeHandle &shape, Point3 pivot,
                                          double factor);
std::unique_ptr<ShapeHandle> shape_mirrored(const ShapeHandle &shape,
                                            Point3 plane_origin,
                                            Point3 plane_normal);

std::size_t face_count(const ShapeHandle &shape);
std::size_t edge_count(const ShapeHandle &shape);
std::size_t solid_count(const ShapeHandle &shape);
std::unique_ptr<ShapeHandle> solid_at(const ShapeHandle &shape,
                                      std::size_t index);
Point3 face_center_of_mass(const ShapeHandle &shape, std::size_t index);
double face_area(const ShapeHandle &shape, std::size_t index);
Point3 face_normal_at(const ShapeHandle &shape, std::size_t index, Point3 point);
SurfaceKindRaw face_surface_kind(const ShapeHandle &shape, std::size_t index);
CylinderDataRaw face_cylinder_data(const ShapeHandle &shape, std::size_t index);
bool face_is_reversed(const ShapeHandle &shape, std::size_t index);
bool face_contains_edge(const ShapeHandle &shape, std::size_t face_index,
                        std::size_t edge_index);
Point3 edge_start_point(const ShapeHandle &shape, std::size_t index);
Point3 edge_end_point(const ShapeHandle &shape, std::size_t index);
double edge_length(const ShapeHandle &shape, std::size_t index);
rust::Vec<Point3> edge_polyline(const ShapeHandle &shape, std::size_t index,
                                double deflection);
Bounds shape_aabb(const ShapeHandle &shape);
rust::Vec<RayHitRaw> shape_ray_hits(const ShapeHandle &shape, Point3 origin,
                                    Point3 direction);
MeshRaw mesh_shape(const ShapeHandle &shape, double tolerance);

rust::Vec<std::uint8_t> shape_to_brep_data(const ShapeHandle &shape);
std::unique_ptr<ShapeHandle> shape_from_brep_data(rust::Slice<const std::uint8_t> data);
std::unique_ptr<ShapeHandle> shape_from_step_file(rust::Str path);
std::unique_ptr<ShapeHandle> shape_from_iges_file(rust::Str path);
std::unique_ptr<StepWriterHandle> make_step_writer();
void step_writer_add(StepWriterHandle &writer, const ShapeHandle &shape);
void step_writer_write(StepWriterHandle &writer, rust::Str path);
std::unique_ptr<IgesWriterHandle> make_iges_writer();
void iges_writer_add(IgesWriterHandle &writer, const ShapeHandle &shape);
void iges_writer_write(IgesWriterHandle &writer, rust::Str path);
void shape_to_stl_file(const ShapeHandle &shape, rust::Str path, double tolerance);
std::unique_ptr<ShapeHandle> shape_from_stl_file(rust::Str path);
