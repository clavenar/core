use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let userspace = root.join("userspace");
    build_library("clv_abi", &root.join("crates/clv-abi/src"), &output, &[]);
    build_library("clv", &userspace.join("libclv"), &output, &["clv_abi"]);
    let linker_script = userspace.join("user.ld");
    println!("cargo:rerun-if-changed={}", linker_script.display());
    for name in ["hello", "shell", "echo"] {
        let program = userspace.join(name);
        println!("cargo:rerun-if-changed={}", program.display());
        build_program(&program.join("main.rs"), &linker_script, &output, name);
    }
}

fn target_rustc(output: &Path, dependencies: &[&str]) -> Command {
    let mut command = Command::new(env::var_os("RUSTC").unwrap());
    command
        .args(["--edition=2024", "--target"])
        .arg(env::var_os("TARGET").unwrap())
        .args(["-Copt-level=z", "-Cpanic=abort", "-L"])
        .arg(output);
    for dependency in dependencies {
        command.arg("--extern").arg(dependency);
    }
    command
}

fn build_library(name: &str, directory: &Path, output: &Path, dependencies: &[&str]) {
    println!("cargo:rerun-if-changed={}", directory.display());
    let status = target_rustc(output, dependencies)
        .args(["--crate-name", name, "--crate-type=rlib", "--out-dir"])
        .arg(output)
        .arg(directory.join("lib.rs"))
        .status()
        .expect("could not invoke rustc for a user library");
    assert!(status.success(), "could not build {name}");
}

fn build_program(source: &Path, linker_script: &Path, output: &Path, name: &str) {
    let status = target_rustc(output, &["clv_abi", "clv"])
        .arg(format!("--crate-name=clavenar_{name}"))
        .args([
            "-Clto=fat",
            "-Clink-arg=--no-relax",
            "-Clink-arg=--strip-all",
        ])
        .arg(format!("-Clink-arg=-T{}", linker_script.display()))
        .arg(source)
        .arg("-o")
        .arg(output.join(format!("{name}.elf")))
        .status()
        .expect("could not invoke rustc for the boot user program");
    assert!(status.success(), "could not build the {name} program");
}
