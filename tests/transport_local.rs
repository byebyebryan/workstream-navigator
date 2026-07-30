use std::path::PathBuf;

use wsnav::{
    protocol::CURRENT_PROTOCOL_VERSION,
    transport::{HostClient, LocalEndpoint, SystemCommandRunner},
};

#[test]
fn local_subprocess_uses_the_same_bounded_protocol_service_as_ssh() {
    let temporary = tempfile::tempdir().unwrap();
    let endpoint = LocalEndpoint {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
        state_root: temporary.path().join("state"),
    };
    let client = HostClient::new(SystemCommandRunner);

    let hello = client.hello_local(&endpoint, "test-client").unwrap();
    let snapshot = client.snapshot_local(&endpoint).unwrap();

    assert_eq!(hello.wsnav_version, env!("CARGO_PKG_VERSION"));
    assert!(hello.registry_generation.len() <= 128);
    assert!(snapshot.workstreams.is_empty());
    assert_eq!(CURRENT_PROTOCOL_VERSION, 1);
}
