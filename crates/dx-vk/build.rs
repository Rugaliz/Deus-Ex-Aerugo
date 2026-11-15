//! Compiles every `shaders/*.vert|frag` to SPIR-V in `OUT_DIR` (the `.spv` files are not committed): with `glslc`
//! (shaderc, the Vulkan SDK) when it is installed, else with `glslangValidator` (glslang) followed by `spirv-opt -O`
//! (SPIRV-Tools) when that is installed too.
use std::path::{Path, PathBuf};
use std::process::Command;

fn found(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn main() {
    let dir = PathBuf::from("shaders");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=shaders");
    let glslc = found("glslc");
    let glslang = !glslc && found("glslangValidator");
    assert!(glslc || glslang, "no shader compiler: install glslc (shaderc or the Vulkan SDK) or glslangValidator (glslang)");
    let opt = glslang && found("spirv-opt");
    for entry in std::fs::read_dir(&dir).expect("shaders dir") {
        let path = entry.unwrap().path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "vert" && ext != "frag" {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        let target = out.join(format!("{name}.spv"));
        if glslc {
            run(Command::new("glslc").arg("--target-env=vulkan1.3").arg("-O").arg(&path).arg("-o").arg(&target), name);
        } else {
            run(Command::new("glslangValidator").args(["-V", "--target-env", "vulkan1.3", "-o"]).arg(&target).arg(&path), name);
            if opt {
                run(Command::new("spirv-opt").arg("-O").arg(&target).arg("-o").arg(&target), name);
            }
        }
    }
}

fn run(cmd: &mut Command, name: &str) {
    let tool = Path::new(cmd.get_program()).display().to_string();
    let status = cmd.status().unwrap_or_else(|e| panic!("{tool}: {e}"));
    assert!(status.success(), "{tool} failed on {name}");
}
