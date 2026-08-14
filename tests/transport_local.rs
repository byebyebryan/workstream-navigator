use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use wsnav::{
    domain::ProviderKind,
    protocol::CURRENT_PROTOCOL_VERSION,
    protocol::HostAction,
    provider::codex::profile::ProfileOwnership,
    state::{HostRegistry, IntegrationLifecycle, StateRoot},
    transport::{HostClient, LocalEndpoint, SystemCommandRunner},
};

fn record_ready_codex_observer(root: &StateRoot) {
    HostRegistry::open(root)
        .unwrap()
        .record_codex_integration(
            ProfileOwnership {
                canonical_path: PathBuf::from("/tmp/wsnav-test-observer.json"),
                owner_id: "wsnav-test".to_owned(),
                profile_schema_version: 2,
                hook_executable: PathBuf::from("/tmp/wsnav-test"),
                content_hash: "hash".to_owned(),
            },
            IntegrationLifecycle::Ready,
        )
        .unwrap();
}

#[cfg(unix)]
fn provider_fixture_endpoint(temporary: &tempfile::TempDir, state_root: PathBuf) -> LocalEndpoint {
    use std::os::unix::fs::PermissionsExt;

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    let bin = temporary.path().join("provider-bin");
    fs::create_dir(&bin).unwrap();
    write_executable(
        &bin.join("codex"),
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] || exit 1\nprintf 'codex fixture\\n'\n",
    );
    write_executable(
        &bin.join("tmux"),
        "#!/bin/sh\n[ \"$1\" = \"-V\" ] || exit 1\nprintf 'tmux fixture\\n'\n",
    );
    let wrapper = temporary.path().join("wsnav-provider-fixture");
    write_executable(
        &wrapper,
        &format!(
            "#!/bin/sh\nPATH={}:${{PATH:-}}\nexport PATH\nexec {} \"$@\"\n",
            shell_quote(&bin),
            shell_quote(Path::new(env!("CARGO_BIN_EXE_wsnav"))),
        ),
    );
    LocalEndpoint {
        executable: wrapper,
        state_root,
    }
}

#[cfg(not(unix))]
fn provider_fixture_endpoint(_temporary: &tempfile::TempDir, state_root: PathBuf) -> LocalEndpoint {
    LocalEndpoint {
        executable: PathBuf::from(env!("CARGO_BIN_EXE_wsnav")),
        state_root,
    }
}

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
    assert_eq!(CURRENT_PROTOCOL_VERSION, 18);
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
    let endpoint = provider_fixture_endpoint(&temporary, state_root.clone());
    record_ready_codex_observer(&StateRoot::create(&state_root).unwrap());

    let workstream_id = HostClient::new(SystemCommandRunner)
        .create_local(
            &endpoint,
            HostAction::RegisterCheckout {
                checkout_path: checkout.to_string_lossy().into_owned(),
                provider: ProviderKind::Codex,
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
    let mut registry = HostRegistry::open(&root).unwrap();
    registry
        .set_project_browser_root(&browser_root.to_string_lossy())
        .unwrap();
    registry
        .record_codex_integration(
            ProfileOwnership {
                canonical_path: PathBuf::from("/tmp/wsnav-test-observer.json"),
                owner_id: "wsnav-test".to_owned(),
                profile_schema_version: 2,
                hook_executable: PathBuf::from("/tmp/wsnav-test"),
                content_hash: "hash".to_owned(),
            },
            IntegrationLifecycle::Ready,
        )
        .unwrap();
    let endpoint = provider_fixture_endpoint(&temporary, state_root);
    let client = HostClient::new(SystemCommandRunner);

    let directories = client
        .project_directories_local(&endpoint, "", false)
        .unwrap();

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
                provider: ProviderKind::Codex,
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
