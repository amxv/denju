use std::time::Instant;

use denju_registry::Registry;
use denju_wire::{TeamCreateRequest, team_create_request_hash};
use serde::Serialize;
use uuid::Uuid;

use super::{DATABASE, fanout::SESSION_BEARER, millis, support::docker_psql};

#[derive(Debug, Serialize)]
pub(super) struct TeamScaleReport {
    pub members: usize,
    pub team_detail_ms: u64,
    pub returned_members: usize,
    pub membership_plan: String,
}

pub(super) async fn exercise_team_scale(
    registry: &Registry,
    root: &std::path::Path,
    members: usize,
) -> Result<TeamScaleReport, String> {
    let operation = Uuid::now_v7().to_string();
    let team_name = "loadteam";
    let request_hash =
        team_create_request_hash(&operation, team_name).map_err(|error| error.to_string())?;
    let created = registry
        .create_team(
            SESSION_BEARER,
            &TeamCreateRequest {
                operation_id: operation,
                name: team_name.to_owned(),
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(|error| error.message)?;
    let namespace_id = created.team.namespace_id;
    seed_members(root, &namespace_id, members)?;

    let started = Instant::now();
    let detail = registry
        .team_detail(SESSION_BEARER, "@loadteam")
        .await
        .map_err(|error| error.message)?;
    let team_detail_ms = millis(started.elapsed());
    let returned_members = detail.members.len();
    if returned_members != members + 1 {
        return Err(format!(
            "team detail returned {returned_members} members; expected {} including owner",
            members + 1
        ));
    }
    let membership_plan = docker_psql(
        root,
        DATABASE,
        &format!(
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) SELECT user_id FROM team_memberships WHERE team_namespace_id='{namespace_id}'::uuid ORDER BY user_id LIMIT 200;"
        ),
    )?;
    Ok(TeamScaleReport {
        members,
        team_detail_ms,
        returned_members,
        membership_plan,
    })
}

fn seed_members(root: &std::path::Path, namespace_id: &str, members: usize) -> Result<(), String> {
    let sql = format!(
        r#"WITH seed AS MATERIALIZED (
            SELECT i,uuidv7() AS namespace_id,uuidv7() AS author_id,uuidv7() AS user_id
            FROM generate_series(1,{members}) AS i
         ), namespaces_inserted AS (
            INSERT INTO namespaces(id,slug,kind)
            SELECT namespace_id,format('phase17-member-%s',i),'user' FROM seed
         ), authors_inserted AS (
            INSERT INTO author_principals(id,kind) SELECT author_id,'user' FROM seed
         ), users_inserted AS (
            INSERT INTO users(id,namespace_id,author_principal_id,password_hash,recovery_secret_hash)
            SELECT user_id,namespace_id,author_id,'phase17-load-password-hash',decode(repeat('22',32),'hex') FROM seed
         ), principals_linked AS (
            INSERT INTO author_principal_users(author_principal_id,user_id) SELECT author_id,user_id FROM seed
         )
         INSERT INTO team_memberships(team_namespace_id,user_id,role)
         SELECT '{namespace_id}'::uuid,user_id,'member' FROM seed;"#
    );
    docker_psql(root, DATABASE, &sql).map(|_| ())
}
