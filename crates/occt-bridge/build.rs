const OCCT_LIBS: &[&str] = &[
    "TKMath",
    "TKernel",
    "TKDE",
    "TKFeat",
    "TKGeomBase",
    "TKG2d",
    "TKG3d",
    "TKTopAlgo",
    "TKGeomAlgo",
    "TKBRep",
    "TKPrim",
    "TKDESTEP",
    "TKDEIGES",
    "TKDESTL",
    "TKMesh",
    "TKShHealing",
    "TKFillet",
    "TKBool",
    "TKBO",
    "TKOffset",
    "TKXSBase",
    "TKCAF",
    "TKLCAF",
    "TKXCAF",
    "TKHLR",
];

fn main() {
    let root = occt_sys::occt_path();
    let include = root.join("include");
    let lib = root.join("lib");
    let kernel_archive = if cfg!(target_os = "windows") {
        lib.join("TKernel.lib")
    } else {
        lib.join("libTKernel.a")
    };
    if !kernel_archive.exists() {
        occt_sys::build_occt();
    }

    println!("cargo:rustc-link-search=native={}", lib.display());
    for library in OCCT_LIBS {
        println!("cargo:rustc-link-lib=static={library}");
    }

    let mut build = cxx_build::bridge("src/lib.rs");
    build
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .include(include)
        .include("include")
        .file("cpp/occt_bridge.cpp")
        .compile("free3d-occt-bridge");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=include/occt_bridge.h");
    println!("cargo:rerun-if-changed=cpp/occt_bridge.cpp");
}
