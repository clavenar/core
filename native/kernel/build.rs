use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let program =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../userspace/hello");
    let source = program.join("main.rs");
    let assembly = program.join("hello.S");
    let linker_script = program.join("hello.ld");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("hello.elf");

    for input in [&source, &assembly, &linker_script] {
        println!("cargo:rerun-if-changed={}", input.display());
    }

    let status = Command::new(env::var_os("RUSTC").unwrap())
        .args(["--edition=2024", "--crate-name=clavenar_hello", "--target"])
        .arg(env::var_os("TARGET").unwrap())
        .args([
            "-Cpanic=abort",
            "-Clink-arg=--no-relax",
            "-Clink-arg=--strip-all",
        ])
        .arg(format!("-Clink-arg=-T{}", linker_script.display()))
        .arg(&source)
        .arg("-o")
        .arg(output)
        .status()
        .expect("could not invoke rustc for the boot user program");
    assert!(status.success(), "could not build the boot user program");
}
