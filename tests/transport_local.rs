use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use wsnav::{
    domain::ProviderKind,
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

    let build = client.probe_local(&endpoint).unwrap();
    let hello = client.hello_local(&endpoint, "test-client").unwrap();
    let snapshot = client.snapshot_local(&endpoint).unwrap();
    let operations = client.operations_local(&endpoint).unwrap();

    assert_eq!(hello.wsnav_version, env!("CARGO_PKG_VERSION"));
    assert!(build.ensure_compatible_with_local().is_ok());
    assert!(hello.registry_generation.len() <= 128);
    assert!(snapshot.workstreams.is_empty());
    assert!(operations.operations.is_empty());
    assert_eq!(CURRENT_PROTOCOL_VERSION, 16);
}

#[test]
fn local_subprocess_apply_uses_the_same_revision_guard_as_an_ssh_host() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let root = StateRoot::create(&state_root).unwrap();
    let (workstream_id, attention_revision) = {
        let mut registry = HostRegistry::open(&root).unwrap();
        let registered = registry
            .register_project_root(Path::new("/disposable/repository"), ProviderKind::Codex)
            .unwrap();
        let workstream_id = registered.workstream_id;
        let attention_revision = registry
            .mark_result_attention(
                workstream_id,
                wsnav::domain::ProviderSessionId::codex("session").unwrap(),
                "turn".to_owned(),
            )
            .unwrap()
            .revision
            .value();
        (workstream_id, attention_revision)
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

#[test]
fn local_subprocess_registers_one_existing_checkout_without_returning_its_path() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let checkout = temporary.path().join("checkout");
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&checkout)
            .status()
            .unwrap()
            .success()
    );
    fs::write(checkout.join("README.md"), "# disposable\n").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args([
                "-c",
                "user.name=WSNav Test",
                "-c",
                "user.email=wsnav@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "initial",
            ])
            .status()
            .unwrap()
            .success()
    );
    let endpoint = LocalEndpoint {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
        state_root: state_root.clone(),
    };

    let workstream_id = HostClient::new(SystemCommandRunner)
        .create_local(
            &endpoint,
            HostAction::RegisterCheckout {
                checkout_path: checkout.to_string_lossy().into_owned(),
            },
        )
        .unwrap();
    let snapshot = HostClient::new(SystemCommandRunner)
        .snapshot_local(&endpoint)
        .unwrap();

    assert!(
        snapshot
            .workstreams
            .iter()
            .any(|workstream| workstream.workstream_id == workstream_id)
    );
    assert!(
        snapshot
            .workstreams
            .iter()
            .all(|workstream| workstream.project_display_name != checkout.to_string_lossy())
    );
    assert_eq!(
        HostRegistry::open(&StateRoot::create(&state_root).unwrap())
            .unwrap()
            .workstream_overviews()
            .unwrap()
            .iter()
            .filter(|workstream| workstream.workstream_id == workstream_id)
            .count(),
        1
    );
}

#[test]
fn local_subprocess_browses_and_registers_a_host_private_project_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let browser_root = temporary.path().join("projects");
    let checkout = browser_root.join("picker-target");
    fs::create_dir_all(&checkout).unwrap();
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args(["config", "user.name", "WSNav Test"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args(["config", "user.email", "wsnav@example.invalid"])
            .status()
            .unwrap()
            .success()
    );
    fs::write(checkout.join("README.md"), "picker test\n").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&checkout)
            .args(["commit", "--quiet", "-m", "initial"])
            .status()
            .unwrap()
            .success()
    );
    let root = StateRoot::create(&state_root).unwrap();
    HostRegistry::open(&root)
        .unwrap()
        .set_project_browser_root(&browser_root.to_string_lossy())
        .unwrap();
    let endpoint = LocalEndpoint {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
        state_root,
    };
    let client = HostClient::new(SystemCommandRunner);

    let directories = client.project_directories_local(&endpoint, "").unwrap();

    assert_eq!(directories.root_label, "custom root · projects");
    assert!(
        !directories
            .root_label
            .contains(&*temporary.path().to_string_lossy())
    );
    assert_eq!(
        directories
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.is_git_repository))
            .collect::<Vec<_>>(),
        vec![("picker-target", true)]
    );
    let workstream_id = client
        .create_local(
            &endpoint,
            HostAction::RegisterProjectDirectory {
                relative_path: "picker-target".to_owned(),
            },
        )
        .unwrap();
    assert!(
        client
            .snapshot_local(&endpoint)
            .unwrap()
            .workstreams
            .iter()
            .any(|workstream| workstream.workstream_id == workstream_id)
    );
}

#[test]
fn local_subprocess_assembles_multiple_bounded_snapshot_pages() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let root = StateRoot::create(&state_root).unwrap();
    {
        let mut registry = HostRegistry::open(&root).unwrap();
        for index in 0..33 {
            registry
                .register_project_root(
                    Path::new(&format!("/disposable/repository-{index:02}")),
                    wsnav::domain::ProviderKind::Codex,
                )
                .unwrap();
        }
    }
    let endpoint = LocalEndpoint {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
        state_root,
    };

    let snapshot = HostClient::new(SystemCommandRunner)
        .snapshot_local(&endpoint)
        .unwrap();

    assert_eq!(snapshot.workstreams.len(), 33);
    let identities = snapshot
        .workstreams
        .iter()
        .map(|workstream| workstream.workstream_id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(identities.len(), 33);
    assert_eq!(snapshot.next_cursor, None);
}
