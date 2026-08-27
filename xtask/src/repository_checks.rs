use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use denju_wire::{ApiAuth, ApiRoute, OPENAPI_V1_ROUTES};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

const FIXTURE_CHECKSUMS: &str = "spec/fixtures/checksums.sha256";
const OPENAPI_V1: &str = "spec/wire/openapi-v1.json";

pub(crate) fn check(root: &Path) -> Result<(), String> {
    check_fixture_checksums(root)?;
    check_fixture_coverage(root)?;
    check_sqlx_offline_policy(root)?;
    check_openapi_contract(root)?;
    check_release_version_coherence(root)?;
    check_self_host_release_image_contract(root)?;
    check_automation_authority(root)?;
    println!("repository contracts: passed");
    Ok(())
}

pub(crate) fn update_contract_artifacts(root: &Path) -> Result<(), String> {
    let manifest = fixture_checksum_manifest(root)?;
    fs::write(root.join(FIXTURE_CHECKSUMS), manifest)
        .map_err(|error| format!("failed to update {FIXTURE_CHECKSUMS}: {error}"))?;
    println!("updated {FIXTURE_CHECKSUMS}");
    let openapi = openapi_v1_document()?;
    fs::write(root.join(OPENAPI_V1), openapi)
        .map_err(|error| format!("failed to update {OPENAPI_V1}: {error}"))?;
    println!("updated {OPENAPI_V1}");
    Ok(())
}

fn check_fixture_checksums(root: &Path) -> Result<(), String> {
    let expected_path = root.join(FIXTURE_CHECKSUMS);
    let expected = fs::read_to_string(&expected_path)
        .map_err(|error| format!("failed to read {FIXTURE_CHECKSUMS}: {error}"))?;
    let actual = fixture_checksum_manifest(root)?;
    if normalize_newlines(&expected) == actual {
        Ok(())
    } else {
        Err(format!(
            "spec fixture drift detected; regenerate {FIXTURE_CHECKSUMS} with `cargo xtask contracts --update` after intentionally reviewing changed vectors"
        ))
    }
}

fn fixture_checksum_manifest(root: &Path) -> Result<String, String> {
    let fixture_root = root.join("spec/fixtures");
    let mut files = Vec::new();
    collect_files(&fixture_root, &mut files)?;
    files.retain(|path| path != &root.join(FIXTURE_CHECKSUMS));
    files.sort();

    let mut manifest = String::new();
    for path in files {
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read fixture {}: {error}", path.display()))?;
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        manifest.push_str(&format!("{:x}  {relative}\n", Sha256::digest(bytes)));
    }
    Ok(manifest)
}

fn check_fixture_coverage(root: &Path) -> Result<(), String> {
    let fixture_root = root.join("spec/fixtures");
    let mut fixtures = Vec::new();
    collect_files(&fixture_root, &mut fixtures)?;
    let json_fixtures = fixtures
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();

    let mut rust_files = Vec::new();
    collect_rust_files(&root.join("crates"), &mut rust_files)?;
    let mut source = String::new();
    for path in rust_files {
        source.push_str(
            &fs::read_to_string(&path).map_err(|error| {
                format!("failed to read Rust source {}: {error}", path.display())
            })?,
        );
        source.push('\n');
    }
    for fixture in json_fixtures {
        let name = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("non-UTF8 fixture path: {}", fixture.display()))?;
        if !source.contains(name) {
            return Err(format!(
                "spec fixture {name} is not consumed by a Rust conformance test"
            ));
        }
    }
    Ok(())
}

fn check_sqlx_offline_policy(root: &Path) -> Result<(), String> {
    let mut rust_files = Vec::new();
    collect_rust_files(&root.join("apps"), &mut rust_files)?;
    collect_rust_files(&root.join("crates"), &mut rust_files)?;
    let compile_time_macros = [
        "sqlx::query!(",
        "sqlx::query_as!(",
        "sqlx::query_scalar!(",
        "sqlx::query_file!(",
        "sqlx::query_file_as!(",
        "sqlx::query_file_scalar!(",
    ];
    let mut macro_sites = Vec::new();
    for path in rust_files {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if compile_time_macros
            .iter()
            .any(|needle| source.contains(needle))
        {
            macro_sites.push(path);
        }
    }

    let sqlx = root.join(".sqlx");
    let mut metadata = Vec::new();
    collect_files(&sqlx, &mut metadata)?;
    let has_metadata = metadata.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("query-") && name.ends_with(".json"))
    });
    if !macro_sites.is_empty() && !has_metadata {
        return Err(format!(
            "SQLx compile-time query macros exist but .sqlx offline metadata is absent; first site: {}",
            macro_sites[0].display()
        ));
    }
    if macro_sites.is_empty() && has_metadata {
        return Err(
            "stale .sqlx query metadata exists although the workspace uses only runtime SQLx query APIs"
                .to_owned(),
        );
    }
    Ok(())
}

fn check_openapi_contract(root: &Path) -> Result<(), String> {
    check_server_route_catalog(root)?;
    let expected = fs::read_to_string(root.join(OPENAPI_V1))
        .map_err(|error| format!("failed to read {OPENAPI_V1}: {error}"))?;
    let actual = openapi_v1_document()?;
    if normalize_newlines(&expected) == actual {
        Ok(())
    } else {
        Err(format!(
            "OpenAPI contract drift detected; regenerate {OPENAPI_V1} with `cargo xtask contracts --update` after intentionally reviewing the /v1 route/auth change"
        ))
    }
}

fn check_server_route_catalog(root: &Path) -> Result<(), String> {
    let declared = OPENAPI_V1_ROUTES
        .iter()
        .map(|route| (route.method.as_str().to_owned(), route.path.to_owned()))
        .collect::<BTreeSet<_>>();
    let actual = server_v1_routes(root)?;
    if declared == actual {
        return Ok(());
    }
    let missing = declared
        .difference(&actual)
        .map(|(method, path)| format!("{method} {path}"))
        .collect::<Vec<_>>();
    let undocumented = actual
        .difference(&declared)
        .map(|(method, path)| format!("{method} {path}"))
        .collect::<Vec<_>>();
    Err(format!(
        "denju-wire /v1 route catalog does not match the Axum router; missing from server: [{}]; missing from wire catalog: [{}]",
        missing.join(", "),
        undocumented.join(", ")
    ))
}

fn server_v1_routes(root: &Path) -> Result<BTreeSet<(String, String)>, String> {
    let mut files = vec![root.join("apps/denju-server/src/http.rs")];
    collect_rust_files(&root.join("apps/denju-server/src/http"), &mut files)?;
    files.sort();
    files.dedup();
    let mut routes = BTreeSet::new();
    for path in files {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        parse_axum_routes(&source, &mut routes)?;
    }
    Ok(routes)
}

fn parse_axum_routes(source: &str, output: &mut BTreeSet<(String, String)>) -> Result<(), String> {
    const NEEDLE: &str = ".route(";
    let mut cursor = 0;
    while let Some(relative) = source[cursor..].find(NEEDLE) {
        let call = cursor + relative;
        let body_start = call + NEEDLE.len();
        let body_end = matching_route_call_end(source, body_start)
            .ok_or_else(|| "unterminated Axum .route(...) call".to_owned())?;
        let body = &source[body_start..body_end];
        let trimmed = body.trim_start();
        if let Some(rest) = trimmed.strip_prefix('"')
            && let Some(quote) = rest.find('"')
        {
            let route_path = &rest[..quote];
            if route_path.starts_with("/v1/") {
                if contains_call(body, "get") {
                    output.insert(("get".to_owned(), route_path.to_owned()));
                }
                if contains_call(body, "post") {
                    output.insert(("post".to_owned(), route_path.to_owned()));
                }
            }
        }
        cursor = body_end + 1;
    }
    Ok(())
}

fn matching_route_call_end(source: &str, body_start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 1_u32;
    let mut quote = None;
    let mut escaped = false;
    let mut index = body_start;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
        } else {
            match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

fn contains_call(source: &str, name: &str) -> bool {
    let bytes = source.as_bytes();
    let needle = name.as_bytes();
    let mut start = 0;
    while start + needle.len() <= bytes.len() {
        let Some(relative) = source[start..].find(name) else {
            return false;
        };
        let index = start + relative;
        let before_is_ident =
            index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_');
        let mut after = index + needle.len();
        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
            after += 1;
        }
        if !before_is_ident && after < bytes.len() && bytes[after] == b'(' {
            return true;
        }
        start = index + needle.len();
    }
    false
}

fn openapi_v1_document() -> Result<String, String> {
    let mut paths = Map::<String, Value>::new();
    for route in OPENAPI_V1_ROUTES {
        let operation = openapi_operation(route);
        let entry = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        entry
            .as_object_mut()
            .expect("OpenAPI path item is always an object")
            .insert(route.method.as_str().to_owned(), operation);
    }
    let document = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Denju Registry API",
            "version": "v1",
            "description": "Generated inspection contract for Denju's versioned /v1 method, path, and authentication surface. Exact JSON request/response shapes are the Rust DTOs in denju-wire and the checked spec/wire contract notes; Rust client/server code shares those DTOs directly rather than generating types from OpenAPI."
        },
        "servers": [{ "url": "https://registry.denju.ashray.xyz" }],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "bearerAuth": { "type": "http", "scheme": "bearer", "description": "Installation, user-session, or scoped automation bearer as permitted by the endpoint." },
                "operatorBearer": { "type": "http", "scheme": "bearer", "description": "Registry-operator bearer. End-user credentials are rejected." },
                "recoveryBearer": { "type": "http", "scheme": "bearer", "description": "Deployment recovery bearer. Client and operator credentials are rejected." }
            },
            "schemas": {
                "ApiError": {
                    "type": "object",
                    "required": ["code", "message"],
                    "properties": {
                        "code": {
                            "type": "string",
                            "enum": ["invalid_request", "invalid_request_hash", "operation_conflict", "generation_conflict", "unauthorized", "not_found", "quota_exceeded", "internal", "unavailable"]
                        },
                        "message": { "type": "string" }
                    }
                }
            }
        }
    });
    serde_json::to_string_pretty(&document)
        .map(|mut value| {
            value.push('\n');
            value
        })
        .map_err(|error| format!("failed to serialize OpenAPI contract: {error}"))
}

fn openapi_operation(route: &ApiRoute) -> Value {
    let success = if route.path == "/v1/events" {
        json!({
            "description": "Authenticated server-sent sync hints and keepalives.",
            "content": { "text/event-stream": { "schema": { "type": "string" } } }
        })
    } else {
        json!({
            "description": "Successful response. Exact status and JSON body shape are defined by the shared denju-wire DTO contract."
        })
    };
    let mut operation = json!({
        "operationId": operation_id(route),
        "summary": format!("{} {}", route.method.as_str().to_ascii_uppercase(), route.path),
        "tags": [route_tag(route.path)],
        "x-denju-wire-contract": "denju-wire",
        "responses": {
            "2XX": success,
            "default": {
                "description": "Versioned Denju API error.",
                "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiError" } } }
            }
        }
    });
    if route.method.as_str() == "post" {
        operation["requestBody"] = json!({
            "required": true,
            "description": "Exact fields are defined by the denju-wire request DTO used by this endpoint.",
            "content": {
                "application/json": {
                    "schema": { "type": "object", "additionalProperties": true }
                }
            }
        });
    }
    operation["security"] = match route.auth {
        ApiAuth::Public => json!([]),
        ApiAuth::OptionalBearer => json!([{}, { "bearerAuth": [] }]),
        ApiAuth::Bearer => json!([{ "bearerAuth": [] }]),
        ApiAuth::Operator => json!([{ "operatorBearer": [] }]),
        ApiAuth::Recovery => json!([{ "recoveryBearer": [] }]),
    };
    operation
}

fn operation_id(route: &ApiRoute) -> String {
    let mut id = route.method.as_str().to_owned();
    for part in route.path.trim_start_matches('/').split('/') {
        id.push('_');
        id.push_str(&part.replace('-', "_"));
    }
    id
}

fn route_tag(path: &str) -> &str {
    path.trim_start_matches("/v1/")
        .split('/')
        .next()
        .unwrap_or("registry")
}

fn check_release_version_coherence(root: &Path) -> Result<(), String> {
    let workspace_version = workspace_package_version(root)?;

    let npm: Value = serde_json::from_slice(
        &fs::read(root.join("packages/npm/package.json"))
            .map_err(|error| format!("failed to read packages/npm/package.json: {error}"))?,
    )
    .map_err(|error| format!("failed to parse packages/npm/package.json: {error}"))?;
    let npm_version = npm
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| "packages/npm/package.json is missing a string version".to_owned())?;
    if npm_version != workspace_version {
        return Err(format!(
            "release version drift: Cargo workspace is {workspace_version}, npm package is {npm_version}"
        ));
    }

    let dockerfile = fs::read_to_string(root.join("Dockerfile.vercel"))
        .map_err(|error| format!("failed to read Dockerfile.vercel: {error}"))?;
    let docker_version = dockerfile
        .lines()
        .find_map(|line| line.trim().strip_prefix("ARG DENJU_BUILD_VERSION="))
        .ok_or_else(|| {
            "Dockerfile.vercel is missing ARG DENJU_BUILD_VERSION=<version>".to_owned()
        })?;
    if docker_version != workspace_version {
        return Err(format!(
            "release version drift: Cargo workspace is {workspace_version}, Dockerfile.vercel defaults DENJU_BUILD_VERSION={docker_version}"
        ));
    }

    let release_workflow = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .map_err(|error| format!("failed to read .github/workflows/release.yml: {error}"))?;
    let expected_default = format!("default: {workspace_version}-dry-run");
    if !release_workflow
        .lines()
        .any(|line| line.trim() == expected_default)
    {
        return Err(format!(
            "release version drift: .github/workflows/release.yml must contain `{expected_default}`"
        ));
    }
    check_registry_release_deploy(&release_workflow)?;
    Ok(())
}

fn check_registry_release_deploy(workflow: &str) -> Result<(), String> {
    for required in [
        "VERCEL_TOKEN: ${{ secrets.VERCEL_TOKEN }}",
        "VERCEL_ORG_ID: ${{ secrets.VERCEL_ORG_ID }}",
        "VERCEL_PROJECT_ID: ${{ secrets.VERCEL_PROJECT_ID }}",
        "VERCEL_REGISTRY_ORIGIN: ${{ vars.VERCEL_REGISTRY_ORIGIN }}",
        "DENJU_DATABASE_MIGRATION_URL: ${{ secrets.DENJU_DATABASE_MIGRATION_URL }}",
        r#"cp deploy/vercel.registry.json "$CONTEXT/vercel.json""#,
        "FROM ${REGISTRY}/${IMAGE_NAME}:${{ github.ref_name }}",
        "- name: Verify anonymous server image pull",
        r#"DOCKER_CONFIG="$ANON_DOCKER_CONFIG" docker pull"#,
        r#""${REGISTRY}/${IMAGE_NAME}:${{ github.ref_name }}" migrate"#,
        r#"ORIGIN_DEPLOYMENT_ID="$(node -p 'JSON.parse(process.argv[1]).id' "$ORIGIN_JSON")""#,
        r#"[[ "$ORIGIN_DEPLOYMENT_ID" == "$DEPLOYMENT_ID" ]]"#,
        r#""${VERCEL_REGISTRY_ORIGIN%/}/health/ready""#,
    ] {
        if !workflow.contains(required) {
            return Err(format!(
                "release workflow is missing registry deployment contract: `{required}`"
            ));
        }
    }

    for forbidden in ["VERCEL_ORG_ID: team_", "VERCEL_PROJECT_ID: prj_"] {
        if workflow.contains(forbidden) {
            return Err(format!(
                "release workflow must not hardcode Vercel deployment identity: `{forbidden}`"
            ));
        }
    }

    let image_publish = workflow
        .find("- name: Publish multi-architecture server manifest")
        .ok_or_else(|| "release workflow is missing server image publication".to_owned())?;
    let anonymous_pull = workflow
        .find("- name: Verify anonymous server image pull")
        .ok_or_else(|| {
            "release workflow is missing anonymous server image verification".to_owned()
        })?;
    let registry_deploy = workflow
        .find("- name: Deploy registry from release image")
        .ok_or_else(|| "release workflow is missing registry deployment".to_owned())?;
    let github_release = workflow
        .find("- name: Publish GitHub release")
        .ok_or_else(|| "release workflow is missing GitHub release publication".to_owned())?;
    if !(image_publish < anonymous_pull
        && anonymous_pull < registry_deploy
        && registry_deploy < github_release)
    {
        return Err(
            "release workflow must publish the server image, verify anonymous pulls, deploy the configured registry, then publish the GitHub release"
                .to_owned(),
        );
    }
    Ok(())
}

fn check_self_host_release_image_contract(root: &Path) -> Result<(), String> {
    let compose = fs::read_to_string(root.join("deploy/compose.yml"))
        .map_err(|error| format!("failed to read deploy/compose.yml: {error}"))?;
    let image = "image: ${DENJU_SERVER_IMAGE:-ghcr.io/amxv/denju-server:latest}";
    if compose.matches(image).count() != 2 {
        return Err(
            "deploy/compose.yml must run both migrate and server from the configurable published Denju image"
                .to_owned(),
        );
    }
    if compose.lines().any(|line| line.trim() == "build:") {
        return Err(
            "deploy/compose.yml must consume published server images; source builds belong to the development deployment surface"
                .to_owned(),
        );
    }
    Ok(())
}

fn workspace_package_version(root: &Path) -> Result<String, String> {
    let cargo = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("failed to read Cargo.toml: {error}"))?;
    workspace_package_version_from_manifest(&cargo)
}

fn workspace_package_version_from_manifest(cargo: &str) -> Result<String, String> {
    let mut in_workspace_package = false;
    for line in cargo.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace_package = trimmed == "[workspace.package]";
            continue;
        }
        if !in_workspace_package {
            continue;
        }
        let Some(value) = trimmed.strip_prefix("version") else {
            continue;
        };
        let Some(value) = value.trim_start().strip_prefix('=') else {
            continue;
        };
        let version = value.trim();
        let Some(version) = version
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            return Err("[workspace.package] version must be a quoted string".to_owned());
        };
        if version.is_empty() {
            return Err("[workspace.package] version must not be empty".to_owned());
        }
        return Ok(version.to_owned());
    }
    Err("Cargo.toml is missing [workspace.package] version".to_owned())
}

fn check_automation_authority(root: &Path) -> Result<(), String> {
    for forbidden in ["Makefile", "makefile", "GNUmakefile"] {
        if root.join(forbidden).exists() {
            return Err(format!(
                "{forbidden} introduces a second automation authority; use xtask with thin Justfile aliases"
            ));
        }
    }
    Ok(())
}

fn collect_rust_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    collect_files(directory, output)?;
    output.retain(|path| path.extension().is_some_and(|extension| extension == "rs"));
    Ok(())
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            collect_files(&path, output)?;
        } else if file_type.is_file() {
            output.push(path);
        }
    }
    Ok(())
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axum_route_parser_handles_chained_and_multiline_methods() {
        let source = r#"
            Router::new()
                .route("/v1/example", get(show).post(create))
                .route(
                    "/v1/other",
                    post(update),
                )
                .route("/health/live", get(health));
        "#;
        let mut routes = BTreeSet::new();
        parse_axum_routes(source, &mut routes).unwrap();
        assert_eq!(
            routes,
            BTreeSet::from([
                ("get".to_owned(), "/v1/example".to_owned()),
                ("post".to_owned(), "/v1/example".to_owned()),
                ("post".to_owned(), "/v1/other".to_owned()),
            ])
        );
    }

    #[test]
    fn registry_deploy_requires_secrets_and_exact_release_image_before_publication() {
        let workflow = r#"
- name: Publish multi-architecture server manifest
- name: Verify anonymous server image pull
  run: |
    DOCKER_CONFIG="$ANON_DOCKER_CONFIG" docker pull "${REGISTRY}/${IMAGE_NAME}:${{ github.ref_name }}"
- name: Deploy registry from release image
  env:
    VERCEL_TOKEN: ${{ secrets.VERCEL_TOKEN }}
    VERCEL_ORG_ID: ${{ secrets.VERCEL_ORG_ID }}
    VERCEL_PROJECT_ID: ${{ secrets.VERCEL_PROJECT_ID }}
    VERCEL_REGISTRY_ORIGIN: ${{ vars.VERCEL_REGISTRY_ORIGIN }}
    DENJU_DATABASE_MIGRATION_URL: ${{ secrets.DENJU_DATABASE_MIGRATION_URL }}
  run: |
    "${REGISTRY}/${IMAGE_NAME}:${{ github.ref_name }}" migrate
    cp deploy/vercel.registry.json "$CONTEXT/vercel.json"
    printf '%s\n' "FROM ${REGISTRY}/${IMAGE_NAME}:${{ github.ref_name }}"
    ORIGIN_DEPLOYMENT_ID="$(node -p 'JSON.parse(process.argv[1]).id' "$ORIGIN_JSON")"
    [[ "$ORIGIN_DEPLOYMENT_ID" == "$DEPLOYMENT_ID" ]]
    curl "${VERCEL_REGISTRY_ORIGIN%/}/health/ready"
- name: Publish GitHub release
"#;
        check_registry_release_deploy(workflow).unwrap();

        let wrong_order = workflow.replace(
            "- name: Publish multi-architecture server manifest\n- name: Verify anonymous server image pull",
            "- name: Deploy registry from release image\n- name: Verify anonymous server image pull",
        );
        assert!(check_registry_release_deploy(&wrong_order).is_err());

        let hardcoded = workflow.replace(
            "VERCEL_ORG_ID: ${{ secrets.VERCEL_ORG_ID }}",
            "VERCEL_ORG_ID: team_example",
        );
        assert!(check_registry_release_deploy(&hardcoded).is_err());
    }

    #[test]
    fn workspace_package_version_parser_is_scoped_to_workspace_package() {
        let cargo = r#"
            [package]
            version = "9.9.9"

            [workspace.package]
            version = "1.2.3"

            [workspace.dependencies]
            example = "4.5.6"
        "#;
        assert_eq!(
            workspace_package_version_from_manifest(cargo).unwrap(),
            "1.2.3"
        );
    }
}
