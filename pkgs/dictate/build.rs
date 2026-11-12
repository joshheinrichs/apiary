use std::env;
use std::path::PathBuf;

// Links sherpa-onnx's C API and generates bindings from its header; the store
// path comes from the derivation. bindgen's libclang is rustPlatform.bindgenHook.
fn main() {
    let sherpa = env::var("DICTATE_SHERPA").expect("DICTATE_SHERPA is sherpa-onnx's store path");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-env-changed=DICTATE_SHERPA");
    println!("cargo:rustc-link-search=native={sherpa}/lib");
    println!("cargo:rustc-link-lib=sherpa-onnx-c-api");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{sherpa}/include"))
        .allowlist_item("SherpaOnnx.*")
        .layout_tests(false)
        .generate()
        .expect("unable to generate sherpa-onnx bindings");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out.join("sherpa.rs"))
        .expect("couldn't write sherpa-onnx bindings");
}
