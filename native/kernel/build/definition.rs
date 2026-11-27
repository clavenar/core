use clv_abi::Program;
use clv_abi::agent::{
    DELEGATION_REQUEST_MAX_BYTES, DefinitionId, DefinitionPin, FIND_GREETING_V1, FIND_GREETING_V2,
    FIND_GREETING_V3, FIND_GREETING_V4, FIND_GREETING_V5, FileDelegation, FileToolInvocation,
};
use clv_abi::tool::FileTool;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    name: String,
    revision: u8,
    agent: String,
    #[serde(default)]
    host_mock: bool,
    delegate_with_file: Delegation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delegation {
    program: Option<String>,
    tool: Option<String>,
    argument: String,
    reply_bytes: usize,
    #[serde(default)]
    stdio: bool,
    #[serde(default)]
    mcp: bool,
}

pub(super) fn build(directory: &Path, output: &Path, id: DefinitionId) {
    let stem = format!("{}-v{}", id.intent.name(), id.revision);
    let path = directory.join(format!("{stem}.toml"));
    println!("cargo:rerun-if-changed={}", path.display());
    let source = fs::read_to_string(path).expect("could not read the find-greeting definition");
    let definition: Definition = toml::from_str(&source).expect("invalid find-greeting definition");
    assert_eq!(definition.name, id.intent.name());
    assert_eq!(definition.revision, id.revision);
    assert_eq!(definition.agent, id.intent.program().name());
    assert_eq!(definition.host_mock, id == FIND_GREETING_V5);

    let step = definition.delegate_with_file;
    assert_eq!(
        (step.stdio, step.mcp),
        (
            id == FIND_GREETING_V3,
            matches!(id, FIND_GREETING_V4 | FIND_GREETING_V5)
        ),
        "wrong transport for {stem}"
    );
    let argument = step.argument.as_bytes();
    let reply_bytes = step.reply_bytes;
    let mut buffer = [0; DELEGATION_REQUEST_MAX_BYTES];
    let action = match (id, step.program, step.tool) {
        (FIND_GREETING_V1, Some(name), None) => FileDelegation {
            program: Program::named(name.as_bytes()).expect("unknown definition helper"),
            argument,
            reply_bytes,
        }
        .encode(&mut buffer),
        (
            FIND_GREETING_V2 | FIND_GREETING_V3 | FIND_GREETING_V4 | FIND_GREETING_V5,
            None,
            Some(name),
        ) => FileToolInvocation {
            tool: FileTool::named(name.as_bytes())
                .expect("unknown file tool revision")
                .id,
            argument,
            reply_bytes,
        }
        .encode(&mut buffer),
        _ => panic!("invalid delegation target for {stem}"),
    }
    .expect("definition argument exceeds the IPC limit");
    fs::write(output.join(format!("{stem}.action")), action).unwrap();
    let sha256 = Sha256::digest(source.as_bytes()).into();
    let pin = DefinitionPin {
        id,
        sha256: &sha256,
    };
    fs::write(output.join(format!("{stem}.pin")), pin.encode()).unwrap();
}
