use super::{
    ClientCatalog, ClientHostTransport, CommandRunner, HostClient, HostIdentity, Path,
    RemoteExecutable, STANDARD_REMOTE_EXECUTABLE, SshDestination, SshEndpoint, StateError,
    StateRoot, SystemCommandRunner, WorkstreamId, attach_ssh,
};
use super::{
    cli::HostCommands,
    lifecycle::runtime_status_label,
    local::print_operations,
    model::{AppError, parse_operation, parse_optional_provider, parse_workstream},
};

pub(super) fn host_command(root: &StateRoot, command: HostCommands) -> Result<(), AppError> {
    let mut catalog = ClientCatalog::open(root)?;
    match command {
        HostCommands::List => list_ssh_hosts(&catalog),
        HostCommands::Snapshot { alias } => snapshot_ssh_host(&catalog, &alias),
        HostCommands::Operations { alias } => operations_ssh_host(&catalog, &alias),
        HostCommands::Doctor { alias } => doctor_ssh_host(&catalog, &alias),
        HostCommands::RegisterCheckout {
            alias,
            checkout,
            provider,
        } => register_remote_checkout(
            &catalog,
            &alias,
            &checkout,
            parse_optional_provider(provider.as_deref())?,
        ),
        HostCommands::PrepareObserver { alias } => prepare_remote_observer(&catalog, &alias),
        HostCommands::RemoveObserver { alias } => remove_remote_observer(&catalog, &alias),
        HostCommands::Start {
            alias,
            workstream_id,
            revision,
        } => start_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Recover {
            alias,
            workstream_id,
            revision,
        } => recover_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Park {
            alias,
            workstream_id,
            revision,
        } => park_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Archive {
            alias,
            workstream_id,
            revision,
        } => archive_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Restore {
            alias,
            workstream_id,
            revision,
        } => restore_remote_workstream(&catalog, &alias, &workstream_id, revision),
        HostCommands::Rename {
            alias,
            workstream_id,
            revision,
            name,
        } => rename_remote_workstream(&catalog, &alias, &workstream_id, revision, &name),
        HostCommands::New {
            alias,
            source_workstream_id,
            revision,
            provider,
        } => new_remote_workstream(
            &catalog,
            &alias,
            &source_workstream_id,
            revision,
            parse_optional_provider(provider.as_deref())?,
        ),
        HostCommands::Fork {
            alias,
            source_workstream_id,
            revision,
        } => fork_remote_workstream(&catalog, &alias, &source_workstream_id, revision),
        HostCommands::RecoverOperation {
            alias,
            operation_id,
        } => recover_remote_operation(&catalog, &alias, &operation_id),
        HostCommands::Acknowledge {
            alias,
            workstream_id,
            attention_revision,
        } => acknowledge_remote_workstream(&catalog, &alias, &workstream_id, attention_revision),
        HostCommands::Attach {
            alias,
            workstream_id,
        } => attach_remote_workstream(&catalog, &alias, &workstream_id),
        HostCommands::Reset { alias } => {
            catalog.reset_ssh_host(&alias)?;
            println!("reset SSH host {alias}");
            Ok(())
        }
    }
}

pub(super) fn register_remote(
    root: &StateRoot,
    host: &str,
    destination: Option<&str>,
    executable: Option<&Path>,
) -> Result<(), AppError> {
    let mut catalog = ClientCatalog::open(root)?;
    if destination.is_none()
        && executable.is_none()
        && let Some(existing) = catalog.host(host)?
    {
        let ClientHostTransport::Ssh {
            destination: existing_destination,
        } = existing.transport
        else {
            return Err(AppError::HostIsNotSsh);
        };
        return register_ssh_host(
            &mut catalog,
            host,
            &existing_destination,
            &existing.executable_path,
        );
    }
    let destination = destination.unwrap_or(host);
    let executable = executable.unwrap_or_else(|| Path::new(STANDARD_REMOTE_EXECUTABLE));
    register_ssh_host(&mut catalog, host, destination, executable)
}

fn register_ssh_host(
    catalog: &mut ClientCatalog,
    alias: &str,
    destination: &str,
    executable: &Path,
) -> Result<(), AppError> {
    let endpoint = ssh_endpoint(destination, executable)?;
    let client = HostClient::new(SystemCommandRunner);
    client
        .probe_ssh(&endpoint)?
        .ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    let identity = HostIdentity {
        host_id: hello.host_id,
        registry_generation: hello.registry_generation,
    };
    catalog.register_ssh_host(
        alias,
        &identity,
        executable,
        endpoint.destination.as_str(),
        hello.capabilities,
    )?;
    println!("registered remote host {alias}");
    Ok(())
}

fn list_ssh_hosts(catalog: &ClientCatalog) -> Result<(), AppError> {
    for host in catalog.ssh_hosts()? {
        let ClientHostTransport::Ssh { destination } = host.transport else {
            continue;
        };
        println!("{} {}", host.alias, destination);
    }
    Ok(())
}

fn snapshot_ssh_host(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let snapshot = HostClient::new(SystemCommandRunner).snapshot_ssh(&endpoint)?;
    println!("host: {alias}");
    for workstream in snapshot.workstreams {
        println!(
            "{} {} {}",
            workstream.workstream_id.short(),
            runtime_status_label(workstream.runtime_status),
            workstream.display_name
        );
    }
    Ok(())
}

fn operations_ssh_host(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let operations = HostClient::new(SystemCommandRunner).operations_ssh(&endpoint)?;
    print_operations(operations.operations.into_iter().map(|operation| {
        (
            operation.operation_id,
            operation.kind,
            operation.phase,
            operation.revision,
        )
    }));
    Ok(())
}

fn register_remote_checkout(
    catalog: &ClientCatalog,
    alias: &str,
    checkout: &str,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let capabilities = HostClient::new(SystemCommandRunner)
        .snapshot_ssh(&endpoint)?
        .provider_capabilities;
    let provider =
        crate::provider::select_registration_provider(&capabilities, requested_provider)?;
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::RegisterCheckout {
            checkout_path: checkout.to_owned(),
            provider,
        },
    )?;
    println!("registered workstream {workstream_id}");
    Ok(())
}

fn prepare_remote_observer(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    apply_remote_action(catalog, alias, crate::protocol::HostAction::PrepareObserver)?;
    println!("remote observer profile is ready for native hook review");
    Ok(())
}

fn remove_remote_observer(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    apply_remote_action(catalog, alias, crate::protocol::HostAction::RemoveObserver)?;
    println!("remote observer integration removed; any provider model settings were preserved");
    Ok(())
}

fn doctor_ssh_host(catalog: &ClientCatalog, alias: &str) -> Result<(), AppError> {
    let endpoint = registered_ssh_endpoint(catalog, alias)?;
    let client = HostClient::new(SystemCommandRunner);
    let build = client.probe_ssh(&endpoint)?;
    build.ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    catalog.verify_hello(alias, &hello)?;
    println!("host: {alias}");
    println!("build: {}", build.package_version);
    println!("control ABI: {}", build.control_abi);
    println!("protocol: {}", build.protocol_version);
    println!("host schema: {}", build.host_schema_version);
    println!("release compatibility: ready");
    Ok(())
}

fn start_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Start {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("started remote workstream {workstream_id}");
    Ok(())
}

fn recover_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Recover {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("recovering remote workstream {workstream_id}");
    Ok(())
}

fn park_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Park {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("parked remote workstream {workstream_id}");
    Ok(())
}

fn archive_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Archive {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("archived remote workstream {workstream_id}");
    Ok(())
}

fn restore_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Restore {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
        },
    )?;
    println!("restored remote workstream {workstream_id}");
    Ok(())
}

fn rename_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    revision: i64,
    name: &str,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::Rename {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: revision,
            name: name.to_owned(),
        },
    )?;
    println!("renamed remote workstream {workstream_id}");
    Ok(())
}

fn new_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    source_workstream_id: &str,
    revision: i64,
    requested_provider: Option<crate::domain::ProviderKind>,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let snapshot = HostClient::new(SystemCommandRunner).snapshot_ssh(&endpoint)?;
    let source_workstream_id = parse_workstream(source_workstream_id)?;
    let source = snapshot
        .workstreams
        .iter()
        .find(|workstream| workstream.workstream_id == source_workstream_id)
        .ok_or(StateError::UnknownOpenWorkstream(source_workstream_id))?;
    let provider = crate::provider::select_new_provider(
        &snapshot.provider_capabilities,
        requested_provider,
        source.provider,
    )?;
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::NewWorkstream {
            source_workstream_id,
            expected_revision: revision,
            request_key: uuid::Uuid::new_v4().to_string(),
            provider,
        },
    )?;
    println!("started independent workstream {workstream_id}");
    Ok(())
}

fn fork_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    source_workstream_id: &str,
    revision: i64,
) -> Result<(), AppError> {
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::ForkWorkstream {
            source_workstream_id: parse_workstream(source_workstream_id)?,
            expected_revision: revision,
            request_key: uuid::Uuid::new_v4().to_string(),
        },
    )?;
    println!("forked workstream {workstream_id}");
    Ok(())
}

fn recover_remote_operation(
    catalog: &ClientCatalog,
    alias: &str,
    operation_id: &str,
) -> Result<(), AppError> {
    let operation_id = parse_operation(operation_id)?;
    let workstream_id = create_remote_workstream(
        catalog,
        alias,
        crate::protocol::HostAction::RecoverOperation { operation_id },
    )?;
    println!("recovered operation {operation_id}; workstream {workstream_id}");
    Ok(())
}

fn acknowledge_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
    attention_revision: i64,
) -> Result<(), AppError> {
    apply_remote_action(
        catalog,
        alias,
        crate::protocol::HostAction::AcknowledgeAttention {
            workstream_id: parse_workstream(workstream_id)?,
            expected_revision: attention_revision,
        },
    )?;
    println!("acknowledged remote workstream {workstream_id}");
    Ok(())
}

pub(super) fn attach_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    workstream_id: &str,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let workstream_id = parse_workstream(workstream_id)?;
    let runtime_id = HostClient::new(SystemCommandRunner)
        .snapshot_ssh(&endpoint)?
        .workstreams
        .into_iter()
        .find(|workstream| workstream.workstream_id == workstream_id)
        .and_then(|workstream| workstream.runtime_id)
        .ok_or(AppError::RemoteRuntimeUnavailable)?;
    attach_ssh(&endpoint, runtime_id)?;
    Ok(())
}

fn registered_ssh_endpoint(catalog: &ClientCatalog, alias: &str) -> Result<SshEndpoint, AppError> {
    let host = catalog.host(alias)?.ok_or(AppError::UnknownHostAlias)?;
    let ClientHostTransport::Ssh { destination } = host.transport else {
        return Err(AppError::HostIsNotSsh);
    };
    ssh_endpoint(&destination, &host.executable_path)
}

pub(super) fn checked_ssh_endpoint(
    catalog: &ClientCatalog,
    alias: &str,
) -> Result<SshEndpoint, AppError> {
    let endpoint = registered_ssh_endpoint(catalog, alias)?;
    let client = HostClient::new(SystemCommandRunner);
    checked_ssh_endpoint_with_client(catalog, alias, &client, endpoint)
}

fn checked_ssh_endpoint_with_client<R: CommandRunner>(
    catalog: &ClientCatalog,
    alias: &str,
    client: &HostClient<R>,
    endpoint: SshEndpoint,
) -> Result<SshEndpoint, AppError> {
    client
        .probe_ssh(&endpoint)?
        .ensure_compatible_with_local()?;
    let hello = client.hello_ssh(&endpoint, "wsnav")?;
    catalog.verify_hello(alias, &hello)?;
    Ok(endpoint)
}

fn apply_remote_action(
    catalog: &ClientCatalog,
    alias: &str,
    action: crate::protocol::HostAction,
) -> Result<(), AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    let client = HostClient::new(SystemCommandRunner);
    client.apply_ssh(&endpoint, action)?;
    Ok(())
}

fn create_remote_workstream(
    catalog: &ClientCatalog,
    alias: &str,
    action: crate::protocol::HostAction,
) -> Result<WorkstreamId, AppError> {
    let endpoint = checked_ssh_endpoint(catalog, alias)?;
    Ok(HostClient::new(SystemCommandRunner).create_ssh(&endpoint, action)?)
}

fn ssh_endpoint(destination: &str, executable: &Path) -> Result<SshEndpoint, AppError> {
    let destination = SshDestination::parse(destination)?;
    let executable = executable
        .to_str()
        .ok_or(AppError::RemoteExecutableNotUtf8)
        .and_then(|value| RemoteExecutable::parse(value).map_err(AppError::Transport))?;
    Ok(SshEndpoint::new(destination, executable))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        build_info::{BuildInfo, BuildInfoError},
        domain::HostId,
        protocol::Capabilities,
        transport::{CommandInvocation, CommandResult, TransportError},
    };

    #[derive(Clone)]
    struct ProbeOnlyRunner {
        calls: Arc<Mutex<Vec<CommandInvocation>>>,
    }

    impl CommandRunner for ProbeOnlyRunner {
        fn run(&self, invocation: CommandInvocation) -> Result<CommandResult, TransportError> {
            self.calls.lock().unwrap().push(invocation);
            let mut build = BuildInfo::current();
            build.control_abi = 1;
            Ok(CommandResult {
                success: true,
                stdout: serde_json::to_vec(&build).unwrap(),
            })
        }
    }

    #[test]
    fn checked_endpoint_rejects_abi_before_hello_or_interactive_effect() {
        let temporary = tempfile::tempdir().unwrap();
        let root = StateRoot::create(temporary.path().join("state")).unwrap();
        let mut catalog = ClientCatalog::open(&root).unwrap();
        let identity = HostIdentity {
            host_id: HostId::new(),
            registry_generation: "generation".to_owned(),
        };
        catalog
            .register_ssh_host(
                "snap",
                &identity,
                Path::new("/home/bryan/.local/bin/wsnav"),
                "snap",
                Capabilities {
                    git: true,
                    tmux: true,
                },
            )
            .unwrap();
        let endpoint = registered_ssh_endpoint(&catalog, "snap").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let client = HostClient::new(ProbeOnlyRunner {
            calls: calls.clone(),
        });

        let result = checked_ssh_endpoint_with_client(&catalog, "snap", &client, endpoint);

        assert!(matches!(
            result,
            Err(AppError::BuildInfo(BuildInfoError::ControlAbiMismatch {
                local: 2,
                remote: 1
            }))
        ));
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments[7], "_probe");
    }
}
