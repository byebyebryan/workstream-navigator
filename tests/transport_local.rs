use std::path::PathBuf;

use wsnav::{
    domain::WorkstreamId,
    protocol::CURRENT_PROTOCOL_VERSION,
    protocol::HostAction,
    state::{HostRegistry, StateRoot},
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
    assert_eq!(CURRENT_PROTOCOL_VERSION, 2);
}

#[test]
fn local_subprocess_apply_uses_the_same_revision_guard_as_an_ssh_host() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let root = StateRoot::create(&state_root).unwrap();
    let workstream_id = WorkstreamId::new();
    let attention_revision = {
        let mut registry = HostRegistry::open(&root).unwrap();
        registry
            .mark_result_attention(workstream_id, "session".to_owned(), "turn".to_owned())
            .unwrap()
            .revision
            .value()
    };
    let endpoint = LocalEndpoint {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
        state_root,
    };

    let revision = HostClient::new(SystemCommandRunner)
        .apply_local(
            &endpoint,
            HostAction::AcknowledgeAttention {
                workstream_id,
                expected_revision: attention_revision,
            },
        )
        .unwrap();

    assert_eq!(revision, attention_revision + 1);
    assert_eq!(
        HostRegistry::open(&root)
            .unwrap()
            .attention(workstream_id)
            .unwrap()
            .unwrap()
            .result_unseen_since_revision,
        None
    );
}
