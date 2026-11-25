use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let userspace =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../userspace");
    let linker_script = userspace.join("user.ld");
    println!("cargo:rerun-if-changed={}", linker_script.display());
    for name in ["hello", "shell"] {
        let program = userspace.join(name);
        println!("cargo:rerun-if-changed={}", program.display());
        build_program(&program.join("main.rs"), &linker_script, name);
    }
}

fn build_program(source: &Path, linker_script: &Path, name: &str) {
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(format!("{name}.elf"));

    let status = Command::new(env::var_os("RUSTC").unwrap())
        .arg("--edition=2024")
        .arg(format!("--crate-name=clavenar_{name}"))
        .arg("--target")
        .arg(env::var_os("TARGET").unwrap())
        .args([
            "-Copt-level=z",
            "-Clto=fat",
            "-Cpanic=abort",
            "-Clink-arg=--no-relax",
            "-Clink-arg=--strip-all",
        ])
        .arg(format!("-Clink-arg=-T{}", linker_script.display()))
        .arg(source)
        .arg("-o")
        .arg(output)
        .status()
        .expect("could not invoke rustc for the boot user program");
    assert!(status.success(), "could not build the {name} program");
}
