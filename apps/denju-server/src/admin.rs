use clap::Subcommand;
use denju_registry::Registry;
use denju_wire::{
    AdminQuarantineMutationKind, AdminQuarantineRequest, admin_quarantine_request_hash,
};
use uuid::Uuid;

use crate::ServerConfig;

#[derive(Debug, Subcommand)]
pub(crate) enum AdminCommand {
    Bootstrap {
        #[arg(long)]
        name: String,
    },
    Revoke {
        operator_id: String,
    },
    Reports {
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        cursor: Option<String>,
    },
    Quarantine {
        locator: String,
        #[arg(long = "version")]
        release_version: Option<u64>,
        #[arg(long)]
        reason: String,
    },
    Unquarantine {
        locator: String,
        #[arg(long = "version")]
        release_version: Option<u64>,
    },
}

pub(crate) async fn run(config: &ServerConfig, command: AdminCommand) -> Result<(), String> {
    match command {
        AdminCommand::Bootstrap { name } => {
            let credential = Registry::bootstrap_operator(admin_database_url(config)?, &name)
                .await
                .map_err(api_error)?;
            println!(
                "operator_id={} name={}\noperator_token={}\nStore this token now; Denju cannot show it again.",
                credential.operator_id, credential.name, credential.token
            );
        }
        AdminCommand::Revoke { operator_id } => {
            let outcome = Registry::revoke_operator(admin_database_url(config)?, &operator_id)
                .await
                .map_err(api_error)?;
            println!("revoked_operator={}", outcome.operator_id);
        }
        AdminCommand::Reports { limit, cursor } => {
            let registry = runtime_registry(config).await?;
            let token = operator_token()?;
            let outcome = registry
                .admin_reports(&token, limit, cursor.as_deref())
                .await
                .map_err(api_error)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&outcome).map_err(|error| error.to_string())?
            );
        }
        AdminCommand::Quarantine {
            locator,
            release_version,
            reason,
        } => {
            let registry = runtime_registry(config).await?;
            mutate(
                &registry,
                AdminQuarantineMutationKind::Quarantine,
                &locator,
                release_version,
                reason,
            )
            .await?;
        }
        AdminCommand::Unquarantine {
            locator,
            release_version,
        } => {
            let registry = runtime_registry(config).await?;
            mutate(
                &registry,
                AdminQuarantineMutationKind::Unquarantine,
                &locator,
                release_version,
                String::new(),
            )
            .await?;
        }
    }
    Ok(())
}

async fn runtime_registry(config: &ServerConfig) -> Result<Registry, String> {
    let registry = Registry::connect(config.registry_settings())
        .await
        .map_err(|error| error.to_string())?;
    registry
        .validate_schema()
        .await
        .map_err(|error| error.to_string())?;
    Ok(registry)
}

fn admin_database_url(config: &ServerConfig) -> Result<&str, String> {
    config.database_migration_url.as_deref().ok_or_else(|| {
        "DENJU_DATABASE_MIGRATION_URL is required for operator bootstrap/revoke and must not be present in the ordinary server runtime".to_owned()
    })
}

async fn mutate(
    registry: &Registry,
    kind: AdminQuarantineMutationKind,
    locator: &str,
    release_version: Option<u64>,
    reason: String,
) -> Result<(), String> {
    let token = operator_token()?;
    let target = registry
        .admin_resolve_resource(&token, locator)
        .await
        .map_err(api_error)?;
    let operation_id = Uuid::now_v7().to_string();
    let request_hash = admin_quarantine_request_hash(
        kind,
        &operation_id,
        &target.resource_id,
        target.generation,
        release_version,
        &reason,
    )
    .map_err(|error| error.to_string())?;
    let outcome = registry
        .mutate_quarantine(
            &token,
            kind,
            &AdminQuarantineRequest {
                operation_id,
                resource_id: target.resource_id,
                expected_generation: target.generation,
                release_version,
                reason,
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(api_error)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&outcome).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn operator_token() -> Result<String, String> {
    std::env::var("DENJU_OPERATOR_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "DENJU_OPERATOR_TOKEN is required for this admin command".to_owned())
}

fn api_error(error: denju_wire::ApiError) -> String {
    format!("{:?}: {}", error.code, error.message)
}
