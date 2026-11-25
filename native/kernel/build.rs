use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let abi_source = root.join("crates/clv-abi/src");
    let abi = output.join("libclv_abi.rlib");
    println!("cargo:rerun-if-changed={}", abi_source.display());
    build_abi(&abi_source.join("lib.rs"), &abi);

    let userspace = root.join("userspace");
    let linker_script = userspace.join("user.ld");
    println!("cargo:rerun-if-changed={}", linker_script.display());
    for name in ["hello", "shell", "echo"] {
        let program = userspace.join(name);
        println!("cargo:rerun-if-changed={}", program.display());
        build_program(
            &program.join("main.rs"),
            &linker_script,
            &abi,
            &output,
            name,
        );
    }
}

fn target_rustc() -> Command {
    let mut command = Command::new(env::var_os("RUSTC").unwrap());
    command
        .args(["--edition=2024", "--target"])
        .arg(env::var_os("TARGET").unwrap())
        .args(["-Copt-level=z", "-Cpanic=abort"]);
    command
}

fn build_abi(source: &Path, output: &Path) {
    let status = target_rustc()
        .args(["--crate-name=clv_abi", "--crate-type=rlib"])
        .arg(source)
        .arg("-o")
        .arg(output)
        .status()
        .expect("could not invoke rustc for the user ABI");
    assert!(status.success(), "could not build the user ABI");
}

fn build_program(source: &Path, linker_script: &Path, abi: &Path, output: &Path, name: &str) {
    let status = target_rustc()
        .arg(format!("--crate-name=clavenar_{name}"))
        .arg("--extern")
        .arg(format!("clv_abi={}", abi.display()))
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
