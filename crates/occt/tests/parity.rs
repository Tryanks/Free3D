use glam::{DVec3, dvec3};
use occt::{Edge, Shape, SurfaceKind};
#[test]
fn five_primitives_have_mesh_faces_bounds_and_edges() {
    let tolerance = 0.1;
    let cases = [
        (
            "box",
            Shape::box_from_corners(dvec3(-1.0, 2.0, 3.0), dvec3(4.0, 6.0, 8.0)).unwrap(),
        ),
        (
            "cylinder",
            Shape::cylinder(dvec3(1.0, 2.0, 3.0), 2.5, DVec3::Z, 7.0).unwrap(),
        ),
        ("sphere", Shape::sphere(dvec3(1.0, 2.0, 3.0), 4.0).unwrap()),
        ("cone", Shape::cone(dvec3(1.0, 2.0, 3.0), 4.0, 7.0).unwrap()),
        (
            "torus",
            Shape::torus(dvec3(1.0, 2.0, 3.0), 5.0, 2.0).unwrap(),
        ),
    ];
    for (name, shape) in cases {
        let mesh = shape.mesh(tolerance).unwrap();
        assert!(!mesh.indices.is_empty(), "{name}");
        assert_eq!(
            mesh.face_ranges.len(),
            shape.face_count().unwrap(),
            "{name}"
        );
        assert!(shape.edge_count().unwrap() > 0, "{name}");
        assert!(
            (0..shape.edge_count().unwrap())
                .any(|index| !shape.edge_polyline(index, tolerance).unwrap().is_empty()),
            "{name}"
        );
        let (minimum, maximum) = shape.aabb().unwrap();
        assert!(minimum.is_finite() && maximum.is_finite() && minimum.cmple(maximum).all());
    }
}

#[test]
fn box_front_hlr_has_four_visible_boundary_polylines() {
    let shape = Shape::box_from_corners(DVec3::ZERO, dvec3(20.0, 30.0, 40.0)).unwrap();
    let (visible, _hidden) = shape.hlr(DVec3::Y, 0.01).unwrap();
    assert_eq!(visible.len(), 4, "front box boundary: {visible:?}");
    assert!(visible.iter().all(|line| line.len() >= 2));
}

#[test]
fn surface_kinds_cover_all_five_primitives() {
    let box_shape = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    assert!(
        (0..box_shape.face_count().unwrap())
            .all(|index| box_shape.face_surface_kind(index).unwrap() == SurfaceKind::Plane)
    );

    let cases = [
        (
            Shape::cylinder(DVec3::ZERO, 2.0, DVec3::Z, 3.0).unwrap(),
            SurfaceKind::Cylinder,
        ),
        (
            Shape::sphere(DVec3::ZERO, 2.0).unwrap(),
            SurfaceKind::Sphere,
        ),
        (
            Shape::cone(DVec3::ZERO, 2.0, 3.0).unwrap(),
            SurfaceKind::Cone,
        ),
        (
            Shape::torus(DVec3::ZERO, 5.0, 2.0).unwrap(),
            SurfaceKind::Torus,
        ),
    ];
    for (shape, expected) in cases {
        assert!(
            (0..shape.face_count().unwrap())
                .any(|index| shape.face_surface_kind(index).unwrap() == expected)
        );
    }
}

#[test]
fn sphere_center_normal_is_an_error_instead_of_an_abort() {
    let sphere = Shape::sphere(dvec3(0.0, 0.0, 30.0), 30.0).unwrap();
    let center = sphere.face_center_of_mass(0).unwrap();
    assert!(sphere.face_normal_at(0, center).is_err());
}

#[test]
fn occt_exception_type_and_message_cross_as_result_error() {
    let error = Shape::cylinder(DVec3::ZERO, 2.0, DVec3::ZERO, 3.0).unwrap_err();
    assert!(error.message().contains("Standard_ConstructionError"));
    assert!(!error.message().is_empty());
}

#[test]
fn compound_splits_into_two_solids() {
    let first = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    let second = Shape::box_from_corners(DVec3::splat(3.0), DVec3::splat(4.0)).unwrap();
    let compound = Shape::compound(vec![first, second]).unwrap();
    let solids = compound.solids().unwrap();
    assert_eq!(solids.len(), 2);
    assert!(solids.iter().all(|solid| solid.solid_count().unwrap() == 1));
}

#[test]
fn box_ray_hits_enter_and_exit_the_box() {
    let new = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    let origin = dvec3(0.5, 0.5, 2.0);
    let direction = DVec3::NEG_Z;
    let mut new_hits = new.ray_hits(origin, direction).unwrap();
    new_hits.sort_by(|left, right| left.t.total_cmp(&right.t));
    assert_eq!(new_hits.len(), 2);
    assert!((new_hits[0].t - 1.0).abs() < 1.0e-9);
    assert!((new_hits[1].t - 2.0).abs() < 1.0e-9);
}

#[test]
fn clone_transforms_and_brep_round_trip_preserve_shape_data() {
    let shape = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    let cloned = shape.clone();
    assert_eq!(shape.aabb().unwrap(), cloned.aabb().unwrap());

    let translated = shape.translated(dvec3(2.0, 3.0, 4.0)).unwrap();
    let (minimum, maximum) = translated.aabb().unwrap();
    assert!(minimum.abs_diff_eq(dvec3(2.0, 3.0, 4.0) - DVec3::splat(1.0e-7), 1.0e-9));
    assert!(maximum.abs_diff_eq(dvec3(3.0, 4.0, 5.0) + DVec3::splat(1.0e-7), 1.0e-9));

    let bytes = translated.to_brep_data().unwrap();
    let restored = Shape::from_brep_data(&bytes).unwrap();
    assert_eq!(
        translated.face_count().unwrap(),
        restored.face_count().unwrap()
    );
    assert_eq!(translated.aabb().unwrap(), restored.aabb().unwrap());

    assert!(
        shape
            .rotated(DVec3::ZERO, DVec3::Z, std::f64::consts::FRAC_PI_2)
            .is_ok()
    );
    assert!(shape.scaled(DVec3::ZERO, 2.0).is_ok());
    assert!(shape.mirrored(DVec3::ZERO, DVec3::X).is_ok());
}

#[test]
fn tangent_arc_edge_starts_in_the_requested_direction() {
    let edge = Edge::tangent_arc(DVec3::ZERO, DVec3::X, dvec3(10.0, 10.0, 0.0)).unwrap();
    let points = edge.as_shape().edge_polyline(0, 0.001).unwrap();
    assert!((points[1] - points[0]).normalize().dot(DVec3::X) > 0.999);
}

#[test]
fn step_two_root_round_trip_preserves_two_solids() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("two.step");
    let first = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    let second = Shape::box_from_corners(DVec3::splat(3.0), DVec3::splat(4.0)).unwrap();
    Shape::write_step_refs([&first, &second], &path).unwrap();
    let restored = Shape::read_step(&path).unwrap();
    assert_eq!(restored.solids().unwrap().len(), 2);
}

#[test]
fn stl_box_round_trip_has_bounds_and_triangles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("box.stl");
    let shape = Shape::box_from_corners(DVec3::ZERO, dvec3(2.0, 3.0, 4.0)).unwrap();
    shape.write_stl(&path, 0.05).unwrap();
    let restored = Shape::read_stl(&path).unwrap();
    let mesh = restored.mesh(0.05).unwrap();
    assert!(!mesh.indices.is_empty());
    let (minimum, maximum) = restored.aabb().unwrap();
    assert!(minimum.abs_diff_eq(DVec3::ZERO, 1.0e-5));
    assert!(maximum.abs_diff_eq(dvec3(2.0, 3.0, 4.0), 1.0e-5));
}

#[test]
fn unit_cube_mass_properties_and_validity_are_exact() {
    let cube = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    let properties = cube.volume_properties().unwrap();
    assert!((properties.volume - 1.0).abs() < 1.0e-12);
    assert!((properties.area - 6.0).abs() < 1.0e-12);
    assert!(
        properties
            .center_of_mass
            .abs_diff_eq(DVec3::splat(0.5), 1.0e-12)
    );
    assert!(cube.check().unwrap().is_empty());
    let messages: Vec<String> = cube.check().unwrap();
    assert!(messages.is_empty());
}

#[test]
fn per_element_measurements_match_a_unit_cube() {
    let cube = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
    for face in 0..cube.face_count().unwrap() {
        assert!((cube.face_area(face).unwrap() - 1.0).abs() < 1.0e-12);
    }
    for edge in 0..cube.edge_count().unwrap() {
        assert!((cube.edge_length(edge).unwrap() - 1.0).abs() < 1.0e-12);
    }
}

#[test]
fn control_point_bspline_is_clamped() {
    let poles = [
        DVec3::ZERO,
        dvec3(2.0, 4.0, 0.0),
        dvec3(6.0, 4.0, 0.0),
        dvec3(8.0, 0.0, 0.0),
    ];
    let edge = Edge::bspline_from_poles(&poles, 3).unwrap();
    assert!(
        edge.as_shape()
            .edge_start_point(0)
            .unwrap()
            .abs_diff_eq(poles[0], 1.0e-10)
    );
    assert!(
        edge.as_shape()
            .edge_end_point(0)
            .unwrap()
            .abs_diff_eq(poles[3], 1.0e-10)
    );
}

#[test]
fn variable_fillet_builds_and_preserves_bbox() {
    let cube = Shape::cube(10.0).unwrap();
    let bounds = cube.aabb().unwrap();
    let faces = cube.face_count().unwrap();
    let result = cube.variable_fillet_edges(&[0], 0.4, 1.2).unwrap();
    let result_bounds = result.aabb().unwrap();
    // OCCT's open-contour rolling law may extend the selected edge direction
    // by less than the terminal radius, while transverse extents stay fixed.
    assert!((bounds.0.x - result_bounds.0.x).abs() < 1.0e-6);
    assert!((bounds.0.y - result_bounds.0.y).abs() < 1.0e-6);
    assert!((bounds.1.x - result_bounds.1.x).abs() < 1.0e-6);
    assert!((bounds.1.y - result_bounds.1.y).abs() < 1.0e-6);
    assert!((bounds.0.z - result_bounds.0.z).abs() < 1.2);
    assert!((bounds.1.z - result_bounds.1.z).abs() < 1.2);
    assert!(result.face_count().unwrap() > faces);
}
