use clv_abi::Program;
use clv_abi::agent::{
    DELEGATION_REQUEST_MAX_BYTES, DefinitionPin, FIND_GREETING_V1, FileDelegation,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    name: String,
    revision: u8,
    agent: String,
    delegate_with_file: Delegation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delegation {
    program: String,
    argument: String,
    reply_bytes: usize,
}

pub(super) fn build(path: &Path, output: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    let source = fs::read_to_string(path).expect("could not read the find-greeting definition");
    let definition: Definition = toml::from_str(&source).expect("invalid find-greeting definition");
    let id = FIND_GREETING_V1;
    assert_eq!(definition.name, id.intent.name());
    assert_eq!(definition.revision, id.revision);
    assert_eq!(definition.agent, id.intent.program().name());

    let step = definition.delegate_with_file;
    let request = FileDelegation {
        program: Program::named(step.program.as_bytes()).expect("unknown definition helper"),
        argument: step.argument.as_bytes(),
        reply_bytes: step.reply_bytes,
    };
    let mut buffer = [0; DELEGATION_REQUEST_MAX_BYTES];
    let action = request
        .encode(&mut buffer)
        .expect("definition argument exceeds the IPC limit");
    fs::write(output.join("find-greeting-v1.action"), action).unwrap();
    let sha256 = Sha256::digest(source.as_bytes()).into();
    let pin = DefinitionPin {
        id,
        sha256: &sha256,
    };
    fs::write(output.join("find-greeting-v1.pin"), pin.encode()).unwrap();
}
