#include "occt_bridge.h"

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepBndLib.hxx>
#include <BRepGProp.hxx>
#include <BRepGProp_Face.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepCheck_ListIteratorOfListOfStatus.hxx>
#include <BRepCheck_Result.hxx>
#include <BRepCheck_Status.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRepPrimAPI_MakeWedge.hxx>
#include <BRepBuilderAPI_GTransform.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeSolid.hxx>
#include <BRepBuilderAPI_Sewing.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepAlgoAPI_Section.hxx>
#include <BRepAlgoAPI_Defeaturing.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepOffsetAPI_MakePipe.hxx>
#include <BRepOffsetAPI_MakeFilling.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepOffsetAPI_MakeOffsetShape.hxx>
#include <BRepOffsetAPI_DraftAngle.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepPrimAPI_MakeHalfSpace.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <BinTools.hxx>
#include <Bnd_Box.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <GProp_GProps.hxx>
#include <GeomAPI_ProjectPointOnSurf.hxx>
#include <GeomAPI_Interpolate.hxx>
#include <Geom_BSplineCurve.hxx>
#include <Geom_CylindricalSurface.hxx>
#include <Geom2d_Line.hxx>
#include <GC_MakeArcOfCircle.hxx>
#include <GC_MakeArcOfEllipse.hxx>
#include <GC_MakeEllipse.hxx>
#include <GeomAbs_SurfaceType.hxx>
#include <IntCurvesFace_ShapeIntersector.hxx>
#include <HLRBRep_Algo.hxx>
#include <HLRBRep_HLRToShape.hxx>
#include <HLRAlgo_Projector.hxx>
#include <Poly_Triangle.hxx>
#include <Poly_Triangulation.hxx>
#include <RWStl.hxx>
#include <StlAPI_Writer.hxx>
#include <Standard_Failure.hxx>
#include <STEPControl_Reader.hxx>
#include <STEPControl_Writer.hxx>
#include <IGESControl_Reader.hxx>
#include <IGESControl_Writer.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopExp_Explorer.hxx>
#include <TopExp.hxx>
#include <TopLoc_Location.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Wire.hxx>
#include <TopoDS_Shell.hxx>
#include <TopoDS_Solid.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <gp_Ax1.hxx>
#include <gp_Ax2.hxx>
#include <gp_Ax3.hxx>
#include <gp_Dir.hxx>
#include <gp_Lin.hxx>
#include <gp_Dir2d.hxx>
#include <gp_Pnt2d.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <gp_GTrsf.hxx>
#include <gp_Pln.hxx>
#include <gp_Vec.hxx>
#include <TColgp_HArray1OfPnt.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <TColgp_Array1OfPnt2d.hxx>
#include <TColStd_Array1OfInteger.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <BRepLib.hxx>

#include <cmath>
#include <limits>
#include <cstdint>
#include <sstream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

template <typename Function> decltype(auto) occt_call(Function &&function) {
  try {
    return std::forward<Function>(function)();
  } catch (const Standard_Failure &failure) {
    const char *type = failure.DynamicType()->Name();
    const char *message = failure.GetMessageString();
    throw std::runtime_error(std::string(type == nullptr ? "Standard_Failure" : type) +
                             ": " + (message == nullptr ? "" : message));
  }
}

gp_Pnt point(Point3 value) { return gp_Pnt(value.x, value.y, value.z); }

gp_Dir direction(Point3 value) { return gp_Dir(value.x, value.y, value.z); }

Point3 point3(const gp_XYZ &value) {
  Point3 result;
  result.x = value.X();
  result.y = value.Y();
  result.z = value.Z();
  return result;
}

Point3 point3(const gp_Pnt &value) { return point3(value.XYZ()); }

const char *status_name(BRepCheck_Status status) {
  switch (status) {
  case BRepCheck_NoError: return "NoError";
  case BRepCheck_InvalidPointOnCurve: return "InvalidPointOnCurve";
  case BRepCheck_InvalidPointOnCurveOnSurface: return "InvalidPointOnCurveOnSurface";
  case BRepCheck_InvalidPointOnSurface: return "InvalidPointOnSurface";
  case BRepCheck_No3DCurve: return "No3DCurve";
  case BRepCheck_Multiple3DCurve: return "Multiple3DCurve";
  case BRepCheck_Invalid3DCurve: return "Invalid3DCurve";
  case BRepCheck_NoCurveOnSurface: return "NoCurveOnSurface";
  case BRepCheck_InvalidCurveOnSurface: return "InvalidCurveOnSurface";
  case BRepCheck_InvalidCurveOnClosedSurface: return "InvalidCurveOnClosedSurface";
  case BRepCheck_InvalidSameRangeFlag: return "InvalidSameRangeFlag";
  case BRepCheck_InvalidSameParameterFlag: return "InvalidSameParameterFlag";
  case BRepCheck_InvalidDegeneratedFlag: return "InvalidDegeneratedFlag";
  case BRepCheck_FreeEdge: return "FreeEdge";
  case BRepCheck_InvalidMultiConnexity: return "InvalidMultiConnexity";
  case BRepCheck_InvalidRange: return "InvalidRange";
  case BRepCheck_EmptyWire: return "EmptyWire";
  case BRepCheck_RedundantEdge: return "RedundantEdge";
  case BRepCheck_SelfIntersectingWire: return "SelfIntersectingWire";
  case BRepCheck_NoSurface: return "NoSurface";
  case BRepCheck_InvalidWire: return "InvalidWire";
  case BRepCheck_RedundantWire: return "RedundantWire";
  case BRepCheck_IntersectingWires: return "IntersectingWires";
  case BRepCheck_InvalidImbricationOfWires: return "InvalidImbricationOfWires";
  case BRepCheck_EmptyShell: return "EmptyShell";
  case BRepCheck_RedundantFace: return "RedundantFace";
  case BRepCheck_UnorientableShape: return "UnorientableShape";
  case BRepCheck_NotClosed: return "NotClosed";
  case BRepCheck_NotConnected: return "NotConnected";
  case BRepCheck_SubshapeNotInShape: return "SubshapeNotInShape";
  case BRepCheck_BadOrientation: return "BadOrientation";
  case BRepCheck_BadOrientationOfSubshape: return "BadOrientationOfSubshape";
  case BRepCheck_InvalidPolygonOnTriangulation: return "InvalidPolygonOnTriangulation";
  case BRepCheck_InvalidToleranceValue: return "InvalidToleranceValue";
  case BRepCheck_EnclosedRegion: return "EnclosedRegion";
  case BRepCheck_CheckFail: return "CheckFail";
  }
  return "UnknownStatus";
}

Point3 point3(const gp_Vec &value) { return point3(value.XYZ()); }

TopoDS_Shape nth_subshape(const TopoDS_Shape &shape, TopAbs_ShapeEnum kind,
                          std::size_t index) {
  std::size_t current = 0;
  for (TopExp_Explorer explorer(shape, kind); explorer.More(); explorer.Next()) {
    if (current++ == index) {
      return explorer.Current();
    }
  }
  throw std::out_of_range("topology index is out of range");
}

TopoDS_Wire first_wire(const TopoDS_Shape &shape) {
  if (shape.ShapeType() == TopAbs_WIRE) return TopoDS::Wire(shape);
  for (TopExp_Explorer explorer(shape, TopAbs_WIRE); explorer.More(); explorer.Next()) {
    return TopoDS::Wire(explorer.Current());
  }
  BRepBuilderAPI_MakeWire builder;
  for (TopExp_Explorer explorer(shape, TopAbs_EDGE); explorer.More(); explorer.Next()) {
    builder.Add(TopoDS::Edge(explorer.Current()));
  }
  if (!builder.IsDone()) throw std::runtime_error("shape has no buildable wire");
  return builder.Wire();
}

std::size_t subshape_count(const TopoDS_Shape &shape, TopAbs_ShapeEnum kind) {
  std::size_t count = 0;
  for (TopExp_Explorer explorer(shape, kind); explorer.More(); explorer.Next()) {
    ++count;
  }
  return count;
}

std::unique_ptr<ShapeHandle> transformed(const ShapeHandle &shape,
                                         const gp_Trsf &transform) {
  BRepBuilderAPI_Transform operation(shape.shape, transform, Standard_True);
  return std::make_unique<ShapeHandle>(operation.Shape());
}

} // namespace

MassPropertiesRaw shape_volume_properties(const ShapeHandle &shape) {
  return occt_call([&] {
    GProp_GProps volume_properties;
    GProp_GProps surface_properties;
    BRepGProp::VolumeProperties(shape.shape, volume_properties);
    BRepGProp::SurfaceProperties(shape.shape, surface_properties);
    const gp_Mat matrix = volume_properties.MatrixOfInertia();
    MassPropertiesRaw result;
    result.volume = volume_properties.Mass();
    result.area = surface_properties.Mass();
    result.center = point3(volume_properties.CentreOfMass());
    result.inertia.reserve(9);
    for (int row = 1; row <= 3; ++row) {
      for (int column = 1; column <= 3; ++column) {
        result.inertia.push_back(matrix.Value(row, column));
      }
    }
    return result;
  });
}

rust::Vec<rust::String> shape_check(const ShapeHandle &shape) {
  return occt_call([&] {
    rust::Vec<rust::String> issues;
    BRepCheck_Analyzer analyzer(shape.shape);
    if (analyzer.IsValid()) return issues;
    const TopAbs_ShapeEnum kinds[] = {TopAbs_VERTEX, TopAbs_EDGE, TopAbs_WIRE,
      TopAbs_FACE, TopAbs_SHELL, TopAbs_SOLID, TopAbs_COMPSOLID, TopAbs_COMPOUND};
    for (TopAbs_ShapeEnum kind : kinds) {
      for (TopExp_Explorer explorer(shape.shape, kind); explorer.More(); explorer.Next()) {
        const Handle(BRepCheck_Result) result = analyzer.Result(explorer.Current());
        if (result.IsNull()) continue;
        for (BRepCheck_ListIteratorOfListOfStatus it(result->Status()); it.More(); it.Next()) {
          if (it.Value() != BRepCheck_NoError) issues.emplace_back(status_name(it.Value()));
        }
      }
    }
    if (issues.empty()) issues.emplace_back("InvalidShape");
    return issues;
  });
}

ShapeHandle::ShapeHandle() = default;

ShapeHandle::ShapeHandle(const TopoDS_Shape &value) : shape(value) {}

HlrHandle::HlrHandle(const TopoDS_Shape &visible_shape,
                     const TopoDS_Shape &hidden_shape,
                     const TopoDS_Shape &section_shape)
    : visible(visible_shape), hidden(hidden_shape), section(section_shape) {}

LoftHandle::LoftHandle() : builder(Standard_True) {}
FaceHandle::FaceHandle(const TopoDS_Wire &outer) : builder(outer, Standard_True) {}

std::unique_ptr<ShapeHandle> shape_clone(const ShapeHandle &shape) {
  return occt_call([&] { return std::make_unique<ShapeHandle>(shape.shape); });
}

bool shape_is_null(const ShapeHandle &shape) noexcept { return shape.shape.IsNull(); }

std::unique_ptr<HlrHandle> shape_hlr(const ShapeHandle &shape, Point3 view_dir) {
  return occt_call([&] {
    const gp_Dir direction_value(view_dir.x, view_dir.y, view_dir.z);
    Handle(HLRBRep_Algo) algorithm = new HLRBRep_Algo();
    algorithm->Add(shape.shape);
    algorithm->Projector(HLRAlgo_Projector(
        gp_Ax2(gp_Pnt(0.0, 0.0, 0.0), direction_value)));
    algorithm->Update();
    algorithm->Hide();
    HLRBRep_HLRToShape result(algorithm);
    return std::make_unique<HlrHandle>(result.VCompound(), result.HCompound());
  });
}

std::unique_ptr<HlrHandle> shape_section_hlr(const ShapeHandle &shape,
                                             Point3 plane_origin,
                                             Point3 plane_normal,
                                             Point3 view_dir) {
  return occt_call([&] {
    const gp_Pln plane(point(plane_origin), direction(plane_normal));
    BRepBuilderAPI_MakeFace face(plane, -1.0e5, 1.0e5, -1.0e5, 1.0e5);
    const gp_Pnt keep_point = point(plane_origin).Translated(
        gp_Vec(plane_normal.x, plane_normal.y, plane_normal.z));
    BRepPrimAPI_MakeHalfSpace halfspace(face.Face(), keep_point);
    BRepAlgoAPI_Common clip(shape.shape, halfspace.Solid());
    clip.Build();
    if (!clip.IsDone()) throw Standard_Failure("section halfspace clipping failed");

    BRepAlgoAPI_Section cut(shape.shape, face.Face(), Standard_False);
    cut.Approximation(Standard_True);
    cut.Build();
    if (!cut.IsDone()) throw Standard_Failure("section outline failed");

    const gp_Dir direction_value(view_dir.x, view_dir.y, view_dir.z);
    Handle(HLRBRep_Algo) algorithm = new HLRBRep_Algo();
    algorithm->Add(clip.Shape());
    algorithm->Projector(HLRAlgo_Projector(
        gp_Ax2(gp_Pnt(0.0, 0.0, 0.0), direction_value)));
    algorithm->Update();
    algorithm->Hide();
    HLRBRep_HLRToShape result(algorithm);

    Handle(HLRBRep_Algo) section_algorithm = new HLRBRep_Algo();
    section_algorithm->Add(cut.Shape());
    section_algorithm->Projector(HLRAlgo_Projector(
        gp_Ax2(gp_Pnt(0.0, 0.0, 0.0), direction_value)));
    section_algorithm->Update();
    HLRBRep_HLRToShape section_result(section_algorithm);
    return std::make_unique<HlrHandle>(result.VCompound(), result.HCompound(),
                                       section_result.VCompound());
  });
}

std::unique_ptr<ShapeHandle> hlr_visible(const HlrHandle &hlr) {
  return occt_call([&] { return std::make_unique<ShapeHandle>(hlr.visible); });
}

std::unique_ptr<ShapeHandle> hlr_hidden(const HlrHandle &hlr) {
  return occt_call([&] { return std::make_unique<ShapeHandle>(hlr.hidden); });
}

std::unique_ptr<ShapeHandle> hlr_section(const HlrHandle &hlr) {
  return occt_call([&] { return std::make_unique<ShapeHandle>(hlr.section); });
}

std::unique_ptr<ShapeHandle> make_box(Point3 corner_1, Point3 corner_2) {
  return occt_call([&] {
    const double min_x = std::min(corner_1.x, corner_2.x);
    const double min_y = std::min(corner_1.y, corner_2.y);
    const double min_z = std::min(corner_1.z, corner_2.z);
    const double max_x = std::max(corner_1.x, corner_2.x);
    const double max_y = std::max(corner_1.y, corner_2.y);
    const double max_z = std::max(corner_1.z, corner_2.z);
    BRepPrimAPI_MakeBox builder(gp_Pnt(min_x, min_y, min_z), max_x - min_x,
                                max_y - min_y, max_z - min_z);
    return std::make_unique<ShapeHandle>(builder.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_cylinder(Point3 origin, double radius,
                                           Point3 axis, double height) {
  return occt_call([&] {
    BRepPrimAPI_MakeCylinder builder(gp_Ax2(point(origin), direction(axis)),
                                     radius, height);
    return std::make_unique<ShapeHandle>(builder.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_sphere(Point3 center, double radius) {
  return occt_call([&] {
    BRepPrimAPI_MakeSphere builder(point(center), radius);
    return std::make_unique<ShapeHandle>(builder.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_ellipsoid(Point3 center, double x_radius,
                                            double y_radius, double z_radius) {
  return occt_call([&] {
    if (x_radius <= 0.0 || y_radius <= 0.0 || z_radius <= 0.0)
      throw std::invalid_argument("ellipsoid radii must be positive");
    BRepPrimAPI_MakeSphere sphere(gp_Pnt(0.0, 0.0, 0.0), 1.0);
    gp_GTrsf transform;
    transform.SetValue(1, 1, x_radius);
    transform.SetValue(2, 2, y_radius);
    transform.SetValue(3, 3, z_radius);
    transform.SetTranslationPart(gp_XYZ(center.x, center.y, center.z));
    BRepBuilderAPI_GTransform operation(sphere.Shape(), transform, Standard_True);
    if (!operation.IsDone()) throw std::runtime_error("ellipsoid transform failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_regular_prism(Point3 center, double radius,
                                                std::uint32_t sides, double height) {
  return occt_call([&] {
    if (sides < 3 || radius <= 0.0 || height <= 0.0)
      throw std::invalid_argument("invalid regular prism parameters");
    const double tau = 2.0 * std::acos(-1.0);
    BRepBuilderAPI_MakePolygon polygon;
    for (std::uint32_t index = 0; index < sides; ++index) {
      const double angle = tau * static_cast<double>(index) / sides;
      polygon.Add(gp_Pnt(center.x + radius * std::cos(angle),
                         center.y + radius * std::sin(angle), center.z));
    }
    polygon.Close();
    BRepBuilderAPI_MakeFace face(polygon.Wire());
    BRepPrimAPI_MakePrism prism(face.Face(), gp_Vec(0.0, 0.0, height));
    return std::make_unique<ShapeHandle>(prism.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_wedge(Point3 origin, double dx, double dy,
                                        double dz, double top_dx) {
  return occt_call([&] {
    if (dx <= 0.0 || dy <= 0.0 || dz <= 0.0 || top_dx < 0.0 || top_dx > dx)
      throw std::invalid_argument("invalid wedge parameters");
    BRepPrimAPI_MakeWedge wedge(gp_Ax2(point(origin), gp::DZ()), dx, dy, dz,
                                top_dx);
    return std::make_unique<ShapeHandle>(wedge.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_cone(Point3 origin, double bottom_radius,
                                       double height) {
  return occt_call([&] {
    BRepPrimAPI_MakeCone builder(gp_Ax2(point(origin), gp::DZ()), bottom_radius,
                                 0.0, height);
    return std::make_unique<ShapeHandle>(builder.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_cone_axis(Point3 origin, double bottom_radius,
                                            double top_radius, Point3 axis,
                                            double height) {
  return occt_call([&] {
    BRepPrimAPI_MakeCone builder(gp_Ax2(point(origin), direction(axis)),
                                 bottom_radius, top_radius, height);
    return std::make_unique<ShapeHandle>(builder.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_torus(Point3 center, double major_radius,
                                        double minor_radius) {
  return occt_call([&] {
    const double pi = std::acos(-1.0);
    BRepPrimAPI_MakeTorus builder(gp_Ax2(point(center), gp::DZ()), major_radius,
                                  minor_radius, -pi, pi, 2.0 * pi);
    return std::make_unique<ShapeHandle>(builder.Shape());
  });
}

std::unique_ptr<ShapeHandle> make_compound() {
  return occt_call([] {
    TopoDS_Compound compound;
    BRep_Builder builder;
    builder.MakeCompound(compound);
    return std::make_unique<ShapeHandle>(compound);
  });
}

void compound_add(ShapeHandle &compound, const ShapeHandle &child) {
  occt_call([&] {
    BRep_Builder builder;
    builder.Add(compound.shape, child.shape);
  });
}

std::unique_ptr<ShapeHandle> make_segment(Point3 start, Point3 end) {
  return occt_call([&] { return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(point(start), point(end)).Edge()); });
}

std::unique_ptr<ShapeHandle> make_circle(Point3 center, Point3 normal, double radius) {
  return occt_call([&] {
    gp_Circ circle(gp_Ax2(point(center), direction(normal)), radius);
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(circle).Edge());
  });
}

std::unique_ptr<ShapeHandle> make_three_point_arc(Point3 start, Point3 middle, Point3 end) {
  return occt_call([&] {
    GC_MakeArcOfCircle arc(point(start), point(middle), point(end));
    if (!arc.IsDone()) throw std::runtime_error("three-point arc construction failed");
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(arc.Value()).Edge());
  });
}

std::unique_ptr<ShapeHandle> make_tangent_arc(Point3 start, Point3 tangent, Point3 end) {
  return occt_call([&] {
    GC_MakeArcOfCircle arc(point(start), gp_Vec(tangent.x, tangent.y, tangent.z), point(end));
    if (!arc.IsDone()) throw std::runtime_error("tangent arc construction failed");
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(arc.Value()).Edge());
  });
}

std::unique_ptr<ShapeHandle> make_ellipse(Point3 center, Point3 normal,
                                          Point3 major_direction,
                                          double major_radius, double minor_radius) {
  return occt_call([&] {
    GC_MakeEllipse ellipse(gp_Ax2(point(center), direction(normal), direction(major_direction)),
                           major_radius, minor_radius);
    if (!ellipse.IsDone()) throw std::runtime_error("ellipse construction failed");
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(ellipse.Value()).Edge());
  });
}

std::unique_ptr<ShapeHandle> make_ellipse_arc(Point3 center, Point3 normal,
                                              Point3 major_direction,
                                              double major_radius, double minor_radius,
                                              double start_angle, double end_angle) {
  return occt_call([&] {
    gp_Elips ellipse(gp_Ax2(point(center), direction(normal), direction(major_direction)),
                     major_radius, minor_radius);
    GC_MakeArcOfEllipse arc(ellipse, start_angle, end_angle, true);
    if (!arc.IsDone()) throw std::runtime_error("ellipse arc construction failed");
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(arc.Value()).Edge());
  });
}

std::unique_ptr<ShapeHandle> make_spline(rust::Slice<const Point3> points) {
  return occt_call([&] {
    if (points.size() < 2) throw std::runtime_error("spline needs at least two points");
    Handle(TColgp_HArray1OfPnt) values = new TColgp_HArray1OfPnt(1, points.size());
    for (std::size_t index = 0; index < points.size(); ++index)
      values->SetValue(index + 1, point(points[index]));
    GeomAPI_Interpolate interpolate(values, Standard_False, 1.0e-7);
    interpolate.Perform();
    if (!interpolate.IsDone()) throw std::runtime_error("spline interpolation failed");
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(interpolate.Curve()).Edge());
  });
}

std::unique_ptr<ShapeHandle>
make_bspline_poles(rust::Slice<const Point3> poles, std::uint8_t degree) {
  return occt_call([&] {
    if (degree < 1 || poles.size() <= degree)
      throw std::runtime_error("B-spline needs more poles than its degree");
    TColgp_Array1OfPnt values(1, static_cast<Standard_Integer>(poles.size()));
    for (std::size_t index = 0; index < poles.size(); ++index)
      values.SetValue(static_cast<Standard_Integer>(index + 1), point(poles[index]));
    const Standard_Integer spans = static_cast<Standard_Integer>(poles.size() - degree);
    TColStd_Array1OfReal knots(1, spans + 1);
    TColStd_Array1OfInteger multiplicities(1, spans + 1);
    for (Standard_Integer index = 1; index <= spans + 1; ++index) {
      knots.SetValue(index, static_cast<double>(index - 1));
      multiplicities.SetValue(index,
          (index == 1 || index == spans + 1) ? degree + 1 : 1);
    }
    Handle(Geom_BSplineCurve) curve = new Geom_BSplineCurve(
        values, knots, multiplicities, degree, Standard_False);
    return std::make_unique<ShapeHandle>(BRepBuilderAPI_MakeEdge(curve).Edge());
  });
}

std::unique_ptr<ShapeHandle> make_helix_wire(Point3 origin, Point3 axis,
                                             double radius, double pitch,
                                             double turns, bool left_handed) {
  return occt_call([&] {
    if (radius <= 0.0 || pitch <= 0.0 || turns <= 0.0)
      throw std::invalid_argument("helix parameters must be positive");
    const double tau = 2.0 * std::acos(-1.0);
    const double handed = left_handed ? -1.0 : 1.0;
    const double rise_per_radian = pitch / tau;
    const double direction_length =
        std::sqrt(1.0 + rise_per_radian * rise_per_radian);
    Handle(Geom_CylindricalSurface) surface =
        new Geom_CylindricalSurface(gp_Ax3(point(origin), direction(axis)), radius);
    Handle(Geom2d_Line) line = new Geom2d_Line(
        gp_Pnt2d(0.0, 0.0), gp_Dir2d(handed, rise_per_radian));
    const double last = tau * turns * direction_length;
    BRepBuilderAPI_MakeEdge edge(line, surface, 0.0, last);
    if (!edge.IsDone()) throw std::runtime_error("helix edge construction failed");
    TopoDS_Edge result = edge.Edge();
    BRepLib::BuildCurve3d(result);
    BRepBuilderAPI_MakeWire wire(result);
    if (!wire.IsDone()) throw std::runtime_error("helix wire construction failed");
    return std::make_unique<ShapeHandle>(wire.Wire());
  });
}

std::unique_ptr<WireHandle> make_wire() { return occt_call([] { return std::make_unique<WireHandle>(); }); }
void wire_add_edge(WireHandle &wire, const ShapeHandle &edge) {
  occt_call([&] { wire.builder.Add(TopoDS::Edge(edge.shape)); });
}
std::unique_ptr<ShapeHandle> wire_build(WireHandle &wire) {
  return occt_call([&] {
    if (!wire.builder.IsDone()) throw std::runtime_error("wire construction failed");
    return std::make_unique<ShapeHandle>(wire.builder.Wire());
  });
}
std::unique_ptr<FaceHandle> make_face(const ShapeHandle &outer) {
  return occt_call([&] { return std::make_unique<FaceHandle>(first_wire(outer.shape)); });
}
void face_add_hole(FaceHandle &face, const ShapeHandle &hole) {
  occt_call([&] { face.builder.Add(first_wire(hole.shape)); });
}
std::unique_ptr<ShapeHandle> face_build(FaceHandle &face) {
  return occt_call([&] {
    if (!face.builder.IsDone()) throw std::runtime_error("face construction failed");
    return std::make_unique<ShapeHandle>(face.builder.Face());
  });
}

std::unique_ptr<ShapeHandle> shape_fuse(const ShapeHandle &left,
                                        const ShapeHandle &right) {
  return occt_call([&] {
    BRepAlgoAPI_Fuse operation(left.shape, right.shape);
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("fuse failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> shape_cut(const ShapeHandle &left,
                                       const ShapeHandle &right) {
  return occt_call([&] {
    BRepAlgoAPI_Cut operation(left.shape, right.shape);
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("cut failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> shape_common(const ShapeHandle &left,
                                          const ShapeHandle &right) {
  return occt_call([&] {
    BRepAlgoAPI_Common operation(left.shape, right.shape);
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("common failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> prism_face(const ShapeHandle &shape,
                                        std::size_t face_index, Point3 vector) {
  return occt_call([&] {
    BRepPrimAPI_MakePrism operation(
        nth_subshape(shape.shape, TopAbs_FACE, face_index),
        gp_Vec(vector.x, vector.y, vector.z));
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> prism_wire(const ShapeHandle &shape, Point3 vector) {
  return occt_call([&] {
    BRepPrimAPI_MakePrism operation(first_wire(shape.shape),
                                    gp_Vec(vector.x, vector.y, vector.z));
    if (!operation.IsDone()) throw std::runtime_error("wire prism failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> revolve_face(const ShapeHandle &shape,
                                          std::size_t face_index,
                                          Point3 axis_origin,
                                          Point3 axis_direction,
                                          double angle_rad) {
  return occt_call([&] {
    BRepPrimAPI_MakeRevol operation(
        nth_subshape(shape.shape, TopAbs_FACE, face_index),
        gp_Ax1(point(axis_origin), direction(axis_direction)), angle_rad);
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> revolve_wire(const ShapeHandle &shape,
                                          Point3 axis_origin,
                                          Point3 axis_direction,
                                          double angle_rad) {
  return occt_call([&] {
    BRepPrimAPI_MakeRevol operation(
        first_wire(shape.shape),
        gp_Ax1(point(axis_origin), direction(axis_direction)), angle_rad);
    if (!operation.IsDone()) throw std::runtime_error("wire revolution failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle> sweep_pipe(const ShapeHandle &profile,
                                        const ShapeHandle &spine) {
  return occt_call([&] {
    const TopoDS_Wire wire = first_wire(spine.shape);
    const TopoDS_Shape face = profile.shape.ShapeType() == TopAbs_FACE
                                  ? profile.shape
                                  : nth_subshape(profile.shape, TopAbs_FACE, 0);
    BRepOffsetAPI_MakePipe operation(wire, face);
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("pipe sweep failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<LoftHandle> make_loft() {
  return occt_call([] { return std::make_unique<LoftHandle>(); });
}

void loft_add_wire(LoftHandle &loft, const ShapeHandle &wire) {
  occt_call([&] { loft.builder.AddWire(first_wire(wire.shape)); });
}

std::unique_ptr<ShapeHandle> loft_build(LoftHandle &loft) {
  return occt_call([&] {
    loft.builder.Build();
    if (!loft.builder.IsDone()) throw std::runtime_error("loft failed");
    return std::make_unique<ShapeHandle>(loft.builder.Shape());
  });
}

std::unique_ptr<ShapeHandle>
fillet_edges(const ShapeHandle &shape, double radius,
             rust::Slice<const std::uint32_t> edge_indices) {
  return occt_call([&] {
    BRepFilletAPI_MakeFillet operation(shape.shape);
    for (std::uint32_t index : edge_indices) {
      operation.Add(radius, TopoDS::Edge(nth_subshape(shape.shape, TopAbs_EDGE, index)));
    }
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("fillet failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle>
variable_fillet_edges(const ShapeHandle &shape,
                      rust::Slice<const std::uint32_t> edge_indices,
                      double start_radius, double end_radius) {
  return occt_call([&] {
    BRepFilletAPI_MakeFillet operation(shape.shape);
    for (std::uint32_t index : edge_indices) {
      const TopoDS_Edge edge = TopoDS::Edge(
          nth_subshape(shape.shape, TopAbs_EDGE, index));
      TColgp_Array1OfPnt2d law(1, 2);
      law.SetValue(1, gp_Pnt2d(0.0, start_radius));
      law.SetValue(2, gp_Pnt2d(1.0, end_radius));
      operation.Add(law, edge);
    }
    operation.Build();
    if (!operation.IsDone())
      throw std::runtime_error("variable fillet operation failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle>
chamfer_edges(const ShapeHandle &shape, double distance,
              rust::Slice<const std::uint32_t> edge_indices) {
  return occt_call([&] {
    BRepFilletAPI_MakeChamfer operation(shape.shape);
    for (std::uint32_t index : edge_indices) {
      operation.Add(distance,
                    TopoDS::Edge(nth_subshape(shape.shape, TopAbs_EDGE, index)));
    }
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("chamfer failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle>
hollow_shape(const ShapeHandle &shape,
             rust::Slice<const std::uint32_t> face_indices, double thickness) {
  return occt_call([&] {
    TopTools_ListOfShape faces;
    for (std::uint32_t index : face_indices) {
      faces.Append(nth_subshape(shape.shape, TopAbs_FACE, index));
    }
    BRepOffsetAPI_MakeThickSolid operation;
    operation.MakeThickSolidByJoin(shape.shape, faces, thickness, 1.0e-3);
    if (!operation.IsDone()) throw std::runtime_error("hollow operation failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle>
draft_faces(const ShapeHandle &shape,
            rust::Slice<const std::uint32_t> face_indices, Point3 draft_direction,
            Point3 neutral_origin, Point3 neutral_normal, double angle_rad) {
  return occt_call([&] {
    if (face_indices.empty()) throw std::invalid_argument("draft needs faces");
    BRepOffsetAPI_DraftAngle operation(shape.shape);
    const gp_Dir pull = direction(draft_direction);
    const gp_Pln neutral(point(neutral_origin), direction(neutral_normal));
    for (std::uint32_t index : face_indices) {
      operation.Add(TopoDS::Face(nth_subshape(shape.shape, TopAbs_FACE, index)),
                    pull, angle_rad, neutral, Standard_True);
    }
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("draft failed");
    return std::make_unique<ShapeHandle>(operation.Shape());
  });
}

std::unique_ptr<ShapeHandle>
patch_face(const ShapeHandle &shape,
           rust::Slice<const std::uint32_t> edge_indices) {
  return occt_call([&] {
    if (edge_indices.empty()) throw std::invalid_argument("patch needs edges");
    std::vector<TopoDS_Edge> edges;
    for (std::uint32_t index : edge_indices) {
      const TopoDS_Edge edge =
          TopoDS::Edge(nth_subshape(shape.shape, TopAbs_EDGE, index));
      bool duplicate = false;
      for (const TopoDS_Edge &existing : edges) {
        if (existing.IsSame(edge)) {
          duplicate = true;
          break;
        }
      }
      if (!duplicate) edges.push_back(edge);
    }
    BRepBuilderAPI_MakeWire wire;
    TopTools_ListOfShape edge_list;
    for (const TopoDS_Edge &edge : edges) edge_list.Append(edge);
    wire.Add(edge_list);
    if (!wire.IsDone()) throw std::runtime_error("patch wire construction failed");
    if (!BRep_Tool::IsClosed(wire.Wire()))
      throw std::runtime_error("patch edges do not form a closed loop");

    BRepBuilderAPI_MakeFace planar(wire.Wire(), Standard_True);
    if (planar.IsDone()) return std::make_unique<ShapeHandle>(planar.Face());

    BRepOffsetAPI_MakeFilling filling;
    for (const TopoDS_Edge &edge : edges) {
      filling.Add(edge, GeomAbs_C0, Standard_True);
    }
    filling.Build();
    if (!filling.IsDone()) throw std::runtime_error("surface filling failed");
    return std::make_unique<ShapeHandle>(filling.Shape());
  });
}

std::unique_ptr<ShapeHandle> stitch_shapes(const ShapeHandle &shapes,
                                           double tolerance) {
  return occt_call([&] {
    if (tolerance <= 0.0) throw std::invalid_argument("sewing tolerance must be positive");
    BRepBuilderAPI_Sewing sewing(tolerance);
    for (TopExp_Explorer explorer(shapes.shape, TopAbs_FACE); explorer.More();
         explorer.Next()) {
      sewing.Add(explorer.Current());
    }
    sewing.Perform();
    TopoDS_Shape result = sewing.SewedShape();
    if (result.IsNull()) throw std::runtime_error("sewing produced a null shape");

    TopoDS_Compound solids;
    BRep_Builder compound_builder;
    compound_builder.MakeCompound(solids);
    std::size_t solid_count = 0;
    for (TopExp_Explorer explorer(result, TopAbs_SHELL); explorer.More(); explorer.Next()) {
      const TopoDS_Shell shell = TopoDS::Shell(explorer.Current());
      BRepBuilderAPI_MakeSolid solid(shell);
      if (!solid.IsDone()) continue;
      const TopoDS_Solid candidate = solid.Solid();
      BRepCheck_Analyzer analyzer(candidate);
      GProp_GProps properties;
      BRepGProp::VolumeProperties(candidate, properties);
      if (!analyzer.IsValid() || std::abs(properties.Mass()) <= 1.0e-12) continue;
      compound_builder.Add(solids, candidate);
      ++solid_count;
    }
    if (solid_count == 1) {
      return std::make_unique<ShapeHandle>(
          nth_subshape(solids, TopAbs_SOLID, 0));
    }
    if (solid_count > 1) return std::make_unique<ShapeHandle>(solids);
    return std::make_unique<ShapeHandle>(result);
  });
}

std::unique_ptr<ShapeHandle> thicken_shape(const ShapeHandle &shape,
                                           double thickness) {
  return occt_call([&] {
    if (std::abs(thickness) <= 1.0e-9)
      throw std::invalid_argument("thickness must be non-zero");
    TopoDS_Shape input = shape.shape;
    if (input.ShapeType() == TopAbs_FACE) {
      const TopoDS_Face face = TopoDS::Face(input);
      const BRepAdaptor_Surface surface(face);
      if (surface.GetType() == GeomAbs_Plane) {
        gp_Dir normal = surface.Plane().Axis().Direction();
        if (face.Orientation() == TopAbs_REVERSED) normal.Reverse();
        BRepPrimAPI_MakePrism prism(face, gp_Vec(normal) * thickness);
        if (!prism.IsDone()) throw std::runtime_error("planar thickening failed");
        return std::make_unique<ShapeHandle>(prism.Shape());
      }
      TopoDS_Shell shell;
      BRep_Builder builder;
      builder.MakeShell(shell);
      builder.Add(shell, input);
      input = shell;
    }
    BRepOffsetAPI_MakeOffsetShape operation;
    operation.PerformByJoin(input, thickness, 1.0e-4);
    if (!operation.IsDone()) throw std::runtime_error("thickening failed");

    BRepBuilderAPI_Sewing sewing(1.0e-4);
    for (TopExp_Explorer explorer(input, TopAbs_FACE); explorer.More(); explorer.Next())
      sewing.Add(explorer.Current());
    for (TopExp_Explorer explorer(operation.Shape(), TopAbs_FACE); explorer.More(); explorer.Next())
      sewing.Add(explorer.Current());

    TopTools_IndexedDataMapOfShapeListOfShape edge_faces;
    TopExp::MapShapesAndAncestors(input, TopAbs_EDGE, TopAbs_FACE, edge_faces);
    for (int index = 1; index <= edge_faces.Extent(); ++index) {
      if (edge_faces.FindFromIndex(index).Extent() != 1) continue;
      const TopoDS_Edge edge = TopoDS::Edge(edge_faces.FindKey(index));
      const TopTools_ListOfShape &generated = operation.Generated(edge);
      TopoDS_Edge offset_edge;
      for (TopTools_ListIteratorOfListOfShape it(generated); it.More(); it.Next()) {
        if (it.Value().ShapeType() == TopAbs_EDGE) {
          offset_edge = TopoDS::Edge(it.Value());
          break;
        }
      }
      if (offset_edge.IsNull()) continue;
      BRepBuilderAPI_MakeWire source_wire(edge);
      BRepBuilderAPI_MakeWire offset_wire(offset_edge);
      BRepOffsetAPI_ThruSections side(Standard_False, Standard_False);
      side.CheckCompatibility(Standard_False);
      side.AddWire(source_wire.Wire());
      side.AddWire(offset_wire.Wire());
      side.Build();
      if (side.IsDone()) sewing.Add(side.Shape());
    }
    sewing.Perform();
    const TopoDS_Shape sewed = sewing.SewedShape();
    for (TopExp_Explorer explorer(sewed, TopAbs_SHELL); explorer.More(); explorer.Next()) {
      BRepBuilderAPI_MakeSolid solid(TopoDS::Shell(explorer.Current()));
      if (!solid.IsDone()) continue;
      const TopoDS_Solid candidate = solid.Solid();
      BRepCheck_Analyzer analyzer(candidate);
      GProp_GProps properties;
      BRepGProp::VolumeProperties(candidate, properties);
      if (analyzer.IsValid() && std::abs(properties.Mass()) > 1.0e-12)
        return std::make_unique<ShapeHandle>(candidate);
    }
    throw std::runtime_error("thickening did not produce a closed solid");
  });
}

std::unique_ptr<ShapeHandle>
delete_faces(const ShapeHandle &shape,
             rust::Slice<const std::uint32_t> face_indices) {
  return occt_call([&] {
    if (face_indices.empty()) throw std::invalid_argument("delete face needs faces");
    BRepAlgoAPI_Defeaturing operation;
    operation.SetShape(shape.shape);
    for (std::uint32_t index : face_indices) {
      operation.AddFaceToRemove(nth_subshape(shape.shape, TopAbs_FACE, index));
    }
    operation.Build();
    if (!operation.IsDone()) throw std::runtime_error("delete-face healing failed");
    TopoDS_Shape result = operation.Shape();
    if (result.IsNull() || subshape_count(result, TopAbs_SOLID) == 0)
      throw std::runtime_error("delete-face healing did not produce a solid");
    return std::make_unique<ShapeHandle>(result);
  });
}

std::unique_ptr<ShapeHandle> shape_translated(const ShapeHandle &shape,
                                              double dx, double dy, double dz) {
  return occt_call([&] {
    gp_Trsf transform;
    transform.SetTranslation(gp_Vec(dx, dy, dz));
    return transformed(shape, transform);
  });
}

std::unique_ptr<ShapeHandle> shape_rotated(const ShapeHandle &shape,
                                           Point3 origin, Point3 axis,
                                           double angle_rad) {
  return occt_call([&] {
    gp_Trsf transform;
    transform.SetRotation(gp_Ax1(point(origin), direction(axis)), angle_rad);
    return transformed(shape, transform);
  });
}

std::unique_ptr<ShapeHandle> shape_scaled(const ShapeHandle &shape, Point3 pivot,
                                          double factor) {
  return occt_call([&] {
    gp_Trsf transform;
    transform.SetScale(point(pivot), factor);
    return transformed(shape, transform);
  });
}

std::unique_ptr<ShapeHandle> shape_mirrored(const ShapeHandle &shape,
                                            Point3 plane_origin,
                                            Point3 plane_normal) {
  return occt_call([&] {
    gp_Trsf transform;
    transform.SetMirror(gp_Ax2(point(plane_origin), direction(plane_normal)));
    return transformed(shape, transform);
  });
}

std::size_t face_count(const ShapeHandle &shape) {
  return occt_call([&] { return subshape_count(shape.shape, TopAbs_FACE); });
}

std::size_t edge_count(const ShapeHandle &shape) {
  return occt_call([&] { return subshape_count(shape.shape, TopAbs_EDGE); });
}

std::size_t solid_count(const ShapeHandle &shape) {
  return occt_call([&] { return subshape_count(shape.shape, TopAbs_SOLID); });
}

std::unique_ptr<ShapeHandle> solid_at(const ShapeHandle &shape,
                                      std::size_t index) {
  return occt_call([&] {
    return std::make_unique<ShapeHandle>(
        nth_subshape(shape.shape, TopAbs_SOLID, index));
  });
}

Point3 face_center_of_mass(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    GProp_GProps properties;
    BRepGProp::SurfaceProperties(
        nth_subshape(shape.shape, TopAbs_FACE, index), properties);
    return point3(properties.CentreOfMass());
  });
}

double face_area(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    GProp_GProps properties;
    BRepGProp::SurfaceProperties(
        nth_subshape(shape.shape, TopAbs_FACE, index), properties);
    return properties.Mass();
  });
}

Point3 face_normal_at(const ShapeHandle &shape, std::size_t index, Point3 query) {
  return occt_call([&] {
    const TopoDS_Face face =
        TopoDS::Face(nth_subshape(shape.shape, TopAbs_FACE, index));
    const Handle(Geom_Surface) surface = BRep_Tool::Surface(face);
    GeomAPI_ProjectPointOnSurf projector(point(query), surface);
    if (projector.NbPoints() == 0) {
      throw std::runtime_error("point cannot be projected onto face surface");
    }
    double u = 0.0;
    double v = 0.0;
    projector.LowerDistanceParameters(u, v);
    gp_Pnt position;
    gp_Vec normal;
    BRepGProp_Face properties(face);
    properties.Normal(u, v, position, normal);
    if (face.Orientation() == TopAbs_REVERSED) {
      normal.Reverse();
    }
    return point3(normal);
  });
}

SurfaceKindRaw face_surface_kind(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    const TopoDS_Face face =
        TopoDS::Face(nth_subshape(shape.shape, TopAbs_FACE, index));
    switch (BRepAdaptor_Surface(face).GetType()) {
    case GeomAbs_Plane:
      return SurfaceKindRaw::Plane;
    case GeomAbs_Cylinder:
      return SurfaceKindRaw::Cylinder;
    case GeomAbs_Sphere:
      return SurfaceKindRaw::Sphere;
    case GeomAbs_Cone:
      return SurfaceKindRaw::Cone;
    case GeomAbs_Torus:
      return SurfaceKindRaw::Torus;
    case GeomAbs_BezierSurface:
      return SurfaceKindRaw::Bezier;
    case GeomAbs_BSplineSurface:
      return SurfaceKindRaw::BSpline;
    default:
      return SurfaceKindRaw::Other;
    }
  });
}

CylinderDataRaw face_cylinder_data(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    const TopoDS_Face face = TopoDS::Face(nth_subshape(shape.shape, TopAbs_FACE, index));
    BRepAdaptor_Surface surface(face);
    if (surface.GetType() != GeomAbs_Cylinder)
      throw std::runtime_error("face is not cylindrical");
    const gp_Cylinder cylinder = surface.Cylinder();
    const gp_Pnt location = cylinder.Location();
    const gp_Dir axis = cylinder.Axis().Direction();
    double minimum = std::numeric_limits<double>::infinity();
    double maximum = -std::numeric_limits<double>::infinity();
    for (TopExp_Explorer explorer(face, TopAbs_VERTEX); explorer.More(); explorer.Next()) {
      const gp_Pnt vertex = BRep_Tool::Pnt(TopoDS::Vertex(explorer.Current()));
      const double parameter = gp_Vec(location, vertex).Dot(gp_Vec(axis));
      minimum = std::min(minimum, parameter);
      maximum = std::max(maximum, parameter);
    }
    if (!std::isfinite(minimum) || maximum - minimum <= Precision::Confusion())
      throw std::runtime_error("cylindrical face has no finite height");
    CylinderDataRaw result;
    result.origin = point3(location.Translated(gp_Vec(axis) * minimum));
    result.axis = point3(axis.XYZ());
    result.radius = cylinder.Radius();
    result.height = maximum - minimum;
    return result;
  });
}

bool face_is_reversed(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    return nth_subshape(shape.shape, TopAbs_FACE, index).Orientation() ==
           TopAbs_REVERSED;
  });
}

bool face_contains_edge(const ShapeHandle &shape, std::size_t face_index,
                        std::size_t edge_index) {
  return occt_call([&] {
    const TopoDS_Shape face = nth_subshape(shape.shape, TopAbs_FACE, face_index);
    const TopoDS_Shape edge = nth_subshape(shape.shape, TopAbs_EDGE, edge_index);
    for (TopExp_Explorer explorer(face, TopAbs_EDGE); explorer.More(); explorer.Next()) {
      if (explorer.Current().IsSame(edge)) return true;
    }
    return false;
  });
}

Point3 edge_start_point(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    const TopoDS_Edge edge = TopoDS::Edge(nth_subshape(shape.shape, TopAbs_EDGE, index));
    Standard_Real first = 0.0;
    Standard_Real last = 0.0;
    const Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, first, last);
    if (curve.IsNull()) throw std::runtime_error("edge has no 3D curve");
    return point3(curve->Value(first));
  });
}

Point3 edge_end_point(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    const TopoDS_Edge edge = TopoDS::Edge(nth_subshape(shape.shape, TopAbs_EDGE, index));
    Standard_Real first = 0.0;
    Standard_Real last = 0.0;
    const Handle(Geom_Curve) curve = BRep_Tool::Curve(edge, first, last);
    if (curve.IsNull()) throw std::runtime_error("edge has no 3D curve");
    return point3(curve->Value(last));
  });
}

double edge_length(const ShapeHandle &shape, std::size_t index) {
  return occt_call([&] {
    GProp_GProps properties;
    BRepGProp::LinearProperties(
        nth_subshape(shape.shape, TopAbs_EDGE, index), properties);
    return properties.Mass();
  });
}

rust::Vec<Point3> edge_polyline(const ShapeHandle &shape, std::size_t index,
                                double deflection) {
  return occt_call([&] {
    rust::Vec<Point3> result;
    const TopoDS_Edge edge =
        TopoDS::Edge(nth_subshape(shape.shape, TopAbs_EDGE, index));
    if (BRep_Tool::Degenerated(edge)) {
      return result;
    }
    BRepAdaptor_Curve curve(edge);
    GCPnts_TangentialDeflection sampler(curve, 0.1, deflection);
    for (int sample = 1; sample <= sampler.NbPoints(); ++sample) {
      result.push_back(point3(sampler.Value(sample)));
    }
    return result;
  });
}

Bounds shape_aabb(const ShapeHandle &shape) {
  return occt_call([&] {
    Bnd_Box box;
    BRepBndLib::Add(shape.shape, box, Standard_True);
    double min_x = 0.0;
    double min_y = 0.0;
    double min_z = 0.0;
    double max_x = 0.0;
    double max_y = 0.0;
    double max_z = 0.0;
    box.Get(min_x, min_y, min_z, max_x, max_y, max_z);
    Bounds bounds;
    bounds.min = point3(gp_XYZ(min_x, min_y, min_z));
    bounds.max = point3(gp_XYZ(max_x, max_y, max_z));
    return bounds;
  });
}

rust::Vec<RayHitRaw> shape_ray_hits(const ShapeHandle &shape, Point3 origin,
                                    Point3 ray_direction) {
  return occt_call([&] {
    rust::Vec<RayHitRaw> result;
    IntCurvesFace_ShapeIntersector intersector;
    intersector.Load(shape.shape, 0.0001);
    intersector.Perform(gp_Lin(point(origin), direction(ray_direction)),
                        -Precision::Infinite(), Precision::Infinite());
    for (int hit_index = 1; hit_index <= intersector.NbPnt(); ++hit_index) {
      const TopoDS_Face &hit_face = intersector.Face(hit_index);
      std::uint32_t face_index = 0;
      bool found = false;
      for (TopExp_Explorer explorer(shape.shape, TopAbs_FACE); explorer.More();
           explorer.Next(), ++face_index) {
        if (explorer.Current().IsSame(hit_face)) {
          found = true;
          break;
        }
      }
      if (!found) {
        throw std::runtime_error("intersection face is absent from explorer order");
      }
      RayHitRaw hit;
      hit.face_index = face_index;
      hit.t = intersector.WParameter(hit_index);
      hit.point = point3(intersector.Pnt(hit_index));
      result.push_back(hit);
    }
    return result;
  });
}

MeshRaw mesh_shape(const ShapeHandle &shape, double tolerance) {
  return occt_call([&] {
    BRepMesh_IncrementalMesh mesher(shape.shape, tolerance, Standard_False, 0.5,
                                    Standard_False);
    if (!mesher.IsDone()) {
      throw std::runtime_error("shape triangulation failed");
    }

    MeshRaw mesh;
    for (TopExp_Explorer explorer(shape.shape, TopAbs_FACE); explorer.More();
         explorer.Next()) {
      const TopoDS_Face face = TopoDS::Face(explorer.Current());
      TopLoc_Location location;
      const Handle(Poly_Triangulation) triangulation =
          BRep_Tool::Triangulation(face, location);
      if (triangulation.IsNull()) {
        throw std::runtime_error("face has no triangulation");
      }

      const std::uint32_t vertex_base =
          static_cast<std::uint32_t>(mesh.positions.size());
      mesh.face_starts.push_back(static_cast<std::uint32_t>(mesh.indices.size()));
      for (int node = 1; node <= triangulation->NbNodes(); ++node) {
        gp_Pnt position = triangulation->Node(node);
        position.Transform(location.Transformation());
        mesh.positions.push_back(point3(position));
        mesh.normals.push_back(Point3{});
      }

      for (int triangle_index = 1;
           triangle_index <= triangulation->NbTriangles(); ++triangle_index) {
        int a = 0;
        int b = 0;
        int c = 0;
        triangulation->Triangle(triangle_index).Get(a, b, c);
        if (face.Orientation() == TopAbs_REVERSED) {
          std::swap(a, c);
        }
        const std::uint32_t ia = vertex_base + static_cast<std::uint32_t>(a - 1);
        const std::uint32_t ib = vertex_base + static_cast<std::uint32_t>(b - 1);
        const std::uint32_t ic = vertex_base + static_cast<std::uint32_t>(c - 1);
        mesh.indices.push_back(ia);
        mesh.indices.push_back(ib);
        mesh.indices.push_back(ic);

        const Point3 &pa = mesh.positions[ia];
        const Point3 &pb = mesh.positions[ib];
        const Point3 &pc = mesh.positions[ic];
        const gp_Vec ab(gp_Pnt(pa.x, pa.y, pa.z), gp_Pnt(pb.x, pb.y, pb.z));
        const gp_Vec ac(gp_Pnt(pa.x, pa.y, pa.z), gp_Pnt(pc.x, pc.y, pc.z));
        gp_Vec normal = ab.Crossed(ac);
        if (normal.SquareMagnitude() > 0.0) {
          normal.Normalize();
          for (std::uint32_t vertex : {ia, ib, ic}) {
            mesh.normals[vertex].x += normal.X();
            mesh.normals[vertex].y += normal.Y();
            mesh.normals[vertex].z += normal.Z();
          }
        }
      }
      mesh.face_ends.push_back(static_cast<std::uint32_t>(mesh.indices.size()));
    }

    for (Point3 &normal : mesh.normals) {
      const double length =
          std::sqrt(normal.x * normal.x + normal.y * normal.y + normal.z * normal.z);
      if (length > 0.0) {
        normal.x /= length;
        normal.y /= length;
        normal.z /= length;
      }
    }
    return mesh;
  });
}

rust::Vec<std::uint8_t> shape_to_brep_data(const ShapeHandle &shape) {
  return occt_call([&] {
    std::ostringstream stream(std::ios::out | std::ios::binary);
    BinTools::Write(shape.shape, stream);
    const std::string bytes = stream.str();
    rust::Vec<std::uint8_t> result;
    result.reserve(bytes.size());
    for (unsigned char byte : bytes) {
      result.push_back(byte);
    }
    return result;
  });
}

std::unique_ptr<ShapeHandle>
shape_from_brep_data(rust::Slice<const std::uint8_t> data) {
  return occt_call([&] {
    const std::string bytes(reinterpret_cast<const char *>(data.data()), data.size());
    std::istringstream stream(bytes, std::ios::in | std::ios::binary);
    TopoDS_Shape shape;
    BinTools::Read(shape, stream);
    if (shape.IsNull()) {
      throw std::runtime_error("BREP data decoded to a null shape");
    }
    return std::make_unique<ShapeHandle>(shape);
  });
}

std::unique_ptr<ShapeHandle> shape_from_step_file(rust::Str path) {
  return occt_call([&] {
    const std::string filename(path);
    STEPControl_Reader reader;
    if (reader.ReadFile(filename.c_str()) != IFSelect_RetDone ||
        reader.TransferRoots() == 0) {
      throw std::runtime_error("temporary STEP conversion read failed");
    }
    return std::make_unique<ShapeHandle>(reader.OneShape());
  });
}

std::unique_ptr<ShapeHandle> shape_from_iges_file(rust::Str path) {
  return occt_call([&] {
    const std::string filename(path);
    IGESControl_Reader reader;
    if (reader.ReadFile(filename.c_str()) != IFSelect_RetDone ||
        reader.TransferRoots() == 0) {
      throw std::runtime_error("IGES conversion read failed");
    }
    return std::make_unique<ShapeHandle>(reader.OneShape());
  });
}

std::unique_ptr<StepWriterHandle> make_step_writer() {
  return occt_call([] { return std::make_unique<StepWriterHandle>(); });
}
void step_writer_add(StepWriterHandle &writer, const ShapeHandle &shape) {
  occt_call([&] {
    if (writer.writer.Transfer(shape.shape, STEPControl_AsIs) != IFSelect_RetDone)
      throw std::runtime_error("STEP shape transfer failed");
  });
}
void step_writer_write(StepWriterHandle &writer, rust::Str path) {
  occt_call([&] {
    const std::string filename(path);
    if (writer.writer.Write(filename.c_str()) != IFSelect_RetDone)
      throw std::runtime_error("STEP file write failed");
  });
}

std::unique_ptr<IgesWriterHandle> make_iges_writer() {
  return occt_call([] { return std::make_unique<IgesWriterHandle>(); });
}
void iges_writer_add(IgesWriterHandle &writer, const ShapeHandle &shape) {
  occt_call([&] {
    if (!writer.writer.AddShape(shape.shape))
      throw std::runtime_error("IGES shape transfer failed");
  });
}
void iges_writer_write(IgesWriterHandle &writer, rust::Str path) {
  occt_call([&] {
    const std::string filename(path);
    if (!writer.writer.Write(filename.c_str()))
      throw std::runtime_error("IGES file write failed");
  });
}

void shape_to_stl_file(const ShapeHandle &shape, rust::Str path, double tolerance) {
  occt_call([&] {
    BRepMesh_IncrementalMesh mesh(shape.shape, tolerance);
    mesh.Perform();
    if (!mesh.IsDone()) throw std::runtime_error("STL meshing failed");
    StlAPI_Writer writer;
    const std::string filename(path);
    if (!writer.Write(shape.shape, filename.c_str())) throw std::runtime_error("STL write failed");
  });
}

std::unique_ptr<ShapeHandle> shape_from_stl_file(rust::Str path) {
  return occt_call([&] {
    const std::string filename(path);
    Handle(Poly_Triangulation) triangulation = RWStl::ReadFile(filename.c_str());
    if (triangulation.IsNull() || triangulation->NbTriangles() == 0)
      throw std::runtime_error("STL contains no triangles");
    BRepBuilderAPI_Sewing sewing(1.0e-6);
    for (Standard_Integer index = 1; index <= triangulation->NbTriangles(); ++index) {
      Standard_Integer a, b, c;
      triangulation->Triangle(index).Get(a, b, c);
      BRepBuilderAPI_MakePolygon polygon(triangulation->Node(a), triangulation->Node(b),
                                         triangulation->Node(c), Standard_True);
      if (polygon.IsDone()) sewing.Add(BRepBuilderAPI_MakeFace(polygon.Wire()).Face());
    }
    sewing.Perform();
    TopoDS_Shape result = sewing.SewedShape();
    if (result.IsNull()) throw std::runtime_error("STL sewing failed");
    if (result.ShapeType() == TopAbs_SHELL) {
      BRepBuilderAPI_MakeSolid solid(TopoDS::Shell(result));
      if (solid.IsDone()) result = solid.Solid();
    }
    return std::make_unique<ShapeHandle>(result);
  });
}
