use std::env;
use std::path::PathBuf;

// The sherpa-onnx store path is substituted in by Nix at build time (see
// default.nix); we link its C API and generate bindings from its header.
// bindgen's libclang comes from rustPlatform.bindgenHook.
const SHERPA: &str = "@sherpa@";

fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rustc-link-search=native={SHERPA}/lib");
    println!("cargo:rustc-link-lib=sherpa-onnx-c-api");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{SHERPA}/include"))
        .allowlist_item("SherpaOnnx.*")
        .layout_tests(false)
        .generate()
        .expect("unable to generate sherpa-onnx bindings");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out.join("sherpa.rs"))
        .expect("couldn't write sherpa-onnx bindings");
}
