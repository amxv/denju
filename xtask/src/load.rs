use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use denju_registry::{Registry, RegistryWake};
use denju_wire::{
    CreateInstallationRequest, SubscriptionMutationKind, SubscriptionMutationRequest,
    SyncKnownResource, SyncReconcileRequest, create_installation_request_hash,
    subscription_request_hash,
};
use reqwest::blocking::Client as BlockingClient;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{runtime::Runtime, time::timeout};
use uuid::Uuid;

mod bench;
mod fanout;
mod local;
mod object_store;
mod seed;
mod support;
mod team_scale;
use fanout::PackFanoutReport;
use local::{
    CliLatencyReport, DaemonRuntimeReport, IsolatedCliHome, benchmark_cli, exercise_daemon_runtime,
    run_cli_setup,
};
use object_store::ObjectStoreConcurrencyReport;
use seed::{SeededPublicSkill, seed_public_catalog};
use support::{
    EnvironmentReport, ServerProcess, configure_database_roles, docker_psql, ensure_infrastructure,
    env_usize, environment_report, millis, p95_ms, registry_settings, release_binary,
    reset_database, run_server_subcommand, wait_ready,
};
use team_scale::TeamScaleReport;

const DATABASE: &str = "denju_phase17_load";
const OWNER: &str = "loadbench";
const SERVER_ONE_PORT: u16 = 17_788;
const SERVER_TWO_PORT: u16 = 17_789;
const RECOVERY_TOKEN: &str = "phase17-load-recovery-token";
const DEFAULT_CATALOG_SIZE: usize = 300;
const DEFAULT_REQUEST_SAMPLES: usize = 200;

#[derive(Debug, Serialize)]
struct LoadReport {
    environment: EnvironmentReport,
    catalog_size: usize,
    cold_start_samples_ms: Vec<u64>,
    cold_start_ms: u64,
    registry_search_p95_ms: f64,
    registry_show_p95_ms: f64,
    single_instance_concurrent_search_p95_ms: f64,
    horizontal_search_p95_ms: f64,
    cli_status_latency: CliLatencyReport,
    cli_search_latency: CliLatencyReport,
    daemon_runtime: DaemonRuntimeReport,
    reconcile: Vec<ReconcileReport>,
    missed_event_reconcile: MissedEventReconcileReport,
    pack_fanout: PackFanoutReport,
    object_store_concurrency: ObjectStoreConcurrencyReport,
    team_scale: TeamScaleReport,
    scale_to_zero_recovery_ms: u64,
    cross_instance_wake_ms: u64,
    listener_reconnected: bool,
    sigterm_exit_ms: u64,
    arbitrary_kill_restart_ms: u64,
    sse_disconnect_observed: bool,
    sse_reconnect_observed: bool,
    outbox_pending_after_recovery: u64,
    search_plan: String,
    outbox_plan: String,
}

#[derive(Debug, Serialize)]
struct ReconcileReport {
    watched_resources: usize,
    p95_ms: f64,
}

#[derive(Debug, Serialize)]
struct MissedEventReconcileReport {
    watched_resources: usize,
    synthetic_missed_events: usize,
    before_p95_ms: f64,
    after_p95_ms: f64,
    before_payload_bytes: usize,
    after_payload_bytes: usize,
}

pub(crate) fn run(root: &Path) -> Result<(), String> {
    let catalog_size = env_usize("DENJU_LOAD_CATALOG_SIZE", DEFAULT_CATALOG_SIZE)?;
    let samples = env_usize("DENJU_LOAD_SAMPLES", DEFAULT_REQUEST_SAMPLES)?;
    if catalog_size < 200 {
        return Err("DENJU_LOAD_CATALOG_SIZE must be at least 200".to_owned());
    }
    if samples < 20 {
        return Err("DENJU_LOAD_SAMPLES must be at least 20".to_owned());
    }

    eprintln!("load: infrastructure");
    ensure_infrastructure(root)?;
    reset_database(root)?;
    super::run(
        "cargo",
        &["build", "--release", "-p", "denju-server", "-p", "denju"],
    )?;
    let server_binary = release_binary("denju-server")?;
    let cli_binary = release_binary("denju")?;
    run_server_subcommand(&server_binary, SERVER_ONE_PORT, "migrate")?;
    configure_database_roles(root)?;

    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    let registry = runtime
        .block_on(Registry::connect(registry_settings(SERVER_ONE_PORT)))
        .map_err(|error| format!("connect load registry: {error}"))?;
    eprintln!("load: seed public catalog ({catalog_size})");
    let seeded = runtime
        .block_on(seed_public_catalog(&registry, OWNER, catalog_size))
        .map_err(|error| format!("seed load catalog: {error}"))?;

    let blocking = BlockingClient::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;

    eprintln!("load: cold starts and HTTP latency");
    let cold_start_samples_ms =
        bench::repeated_cold_starts(&server_binary, &blocking, SERVER_ONE_PORT, 3)?;
    let cold_start_ms = bench::max_ms(&cold_start_samples_ms)?;
    let mut server_one = ServerProcess::start(&server_binary, SERVER_ONE_PORT)?;
    wait_ready(&blocking, SERVER_ONE_PORT)?;

    let search_p95 = benchmark_search(&blocking, SERVER_ONE_PORT, samples)?;
    let show_p95 = benchmark_show(&blocking, SERVER_ONE_PORT, &seeded[0].locator, samples)?;
    let single_instance_concurrent_search_p95_ms =
        bench::concurrent_search_p95(&blocking, SERVER_ONE_PORT, 8, 20)?;

    let mut server_two = ServerProcess::start(&server_binary, SERVER_TWO_PORT)?;
    wait_ready(&blocking, SERVER_TWO_PORT)?;
    let horizontal_p95 = benchmark_horizontal_search(&blocking, samples)?;

    eprintln!("load: isolated normal-use CLI and daemon");
    let (cli_status_latency, cli_search_latency, daemon_runtime) = {
        let cli_home = IsolatedCliHome::create(root)?;
        run_cli_setup(&cli_binary, &cli_home, SERVER_ONE_PORT)?;
        let cli_status_latency = benchmark_cli(&cli_binary, &cli_home, &["status"], 40)?;
        let cli_search_latency = benchmark_cli(
            &cli_binary,
            &cli_home,
            &["search", "benchmark", "--limit", "20"],
            40,
        )?;
        let daemon_runtime = exercise_daemon_runtime(&cli_binary, &cli_home)?;
        (cli_status_latency, cli_search_latency, daemon_runtime)
    };

    eprintln!("load: reconcile and missed-event scaling");
    let reconcile = runtime.block_on(benchmark_reconcile(&registry, &seeded))?;
    let missed_event_reconcile =
        benchmark_missed_event_reconcile(&runtime, &registry, root, &seeded)?;
    let fanout_target = seeded
        .last()
        .ok_or_else(|| "load catalog unexpectedly empty".to_owned())?;
    eprintln!("load: publish and pack fanout");
    let pack_fanout = runtime.block_on(fanout::exercise_pack_fanout(
        &registry,
        root,
        fanout_target,
        64,
    ))?;
    eprintln!("load: object-store concurrency and restart");
    let object_store_concurrency =
        runtime.block_on(object_store::exercise_concurrent_provider(&registry, root))?;
    eprintln!("load: team scale");
    let team_scale = runtime.block_on(team_scale::exercise_team_scale(&registry, root, 500))?;

    eprintln!("load: SSE recycle/reconnect");
    let (sse_disconnect_observed, sse_reconnect_observed) = exercise_sse_reconnect(
        &runtime,
        root,
        &server_binary,
        &blocking,
        &registry,
        &seeded,
        &mut server_two,
    )?;

    eprintln!("load: SIGTERM and arbitrary process death");
    let sigterm_started = Instant::now();
    eprintln!("load: scale-to-zero outbox recovery");
    server_one.terminate()?;
    let sigterm_exit_ms = millis(sigterm_started.elapsed());
    server_one = ServerProcess::start(&server_binary, SERVER_ONE_PORT)?;
    wait_ready(&blocking, SERVER_ONE_PORT)?;

    let kill_started = Instant::now();
    server_one.kill()?;
    server_one = ServerProcess::start(&server_binary, SERVER_ONE_PORT)?;
    wait_ready(&blocking, SERVER_ONE_PORT)?;
    assert_show(&blocking, SERVER_ONE_PORT, &seeded[0].locator)?;
    let arbitrary_kill_restart_ms = millis(kill_started.elapsed());

    server_one.terminate()?;
    server_two.terminate()?;
    insert_recovery_outbox_event(root)?;
    thread::sleep(Duration::from_millis(250));
    let scale_started = Instant::now();
    let _recovery_server_one = ServerProcess::start(&server_binary, SERVER_ONE_PORT)?;
    wait_ready(&blocking, SERVER_ONE_PORT)?;
    drain_outbox(&blocking, SERVER_ONE_PORT)?;
    let scale_to_zero_recovery_ms = millis(scale_started.elapsed());
    let outbox_pending_after_recovery = pending_outbox(root)?;
    if outbox_pending_after_recovery != 0 {
        return Err(format!(
            "outbox recovery left {outbox_pending_after_recovery} pending events"
        ));
    }

    eprintln!("load: cross-instance PostgreSQL wake");
    let _wake_server_two = ServerProcess::start(&server_binary, SERVER_TWO_PORT)?;
    wait_ready(&blocking, SERVER_TWO_PORT)?;
    let observer = runtime
        .block_on(Registry::connect(registry_settings(SERVER_TWO_PORT)))
        .map_err(|error| format!("connect wake observer: {error}"))?;
    runtime.block_on(async { observer.ensure_wake_listener() });
    let mut wake_receiver = observer.subscribe_wakes();
    thread::sleep(Duration::from_millis(150));
    insert_recovery_outbox_event(root)?;
    let wake_started = Instant::now();
    drain_outbox(&blocking, SERVER_ONE_PORT)?;
    let wake = runtime
        .block_on(async { timeout(Duration::from_secs(2), wake_receiver.recv()).await })
        .map_err(|_| "cross-instance wake exceeded 2 seconds".to_owned())?
        .map_err(|error| format!("cross-instance wake receive failed: {error}"))?;
    if !matches!(wake, RegistryWake::ResyncAll) {
        return Err("cross-instance recovery wake was not resync_all".to_owned());
    }
    let cross_instance_wake_ms = millis(wake_started.elapsed());

    eprintln!("load: forced LISTEN reconnect");
    let listener_reconnected = exercise_listener_reconnect(&runtime, root, &observer)?;
    if !listener_reconnected {
        return Err(
            "PostgreSQL wake listener did not reconnect after forced disconnect".to_owned(),
        );
    }

    eprintln!("load: query plans and environment report");
    let search_plan = explain_search(root)?;
    let outbox_plan = explain_outbox(root)?;
    let environment = environment_report(root)?;
    let report = LoadReport {
        environment,
        catalog_size,
        cold_start_samples_ms,
        cold_start_ms,
        registry_search_p95_ms: search_p95,
        registry_show_p95_ms: show_p95,
        single_instance_concurrent_search_p95_ms,
        horizontal_search_p95_ms: horizontal_p95,
        cli_status_latency,
        cli_search_latency,
        daemon_runtime,
        reconcile,
        missed_event_reconcile,
        pack_fanout,
        object_store_concurrency,
        team_scale,
        scale_to_zero_recovery_ms,
        cross_instance_wake_ms,
        listener_reconnected,
        sigterm_exit_ms,
        arbitrary_kill_restart_ms,
        sse_disconnect_observed,
        sse_reconnect_observed,
        outbox_pending_after_recovery,
        search_plan,
        outbox_plan,
    };
    write_report(root, &report)?;
    enforce_targets(&report)?;
    println!("load/stateless harness passed");
    Ok(())
}

fn benchmark_search(client: &BlockingClient, port: u16, samples: usize) -> Result<f64, String> {
    benchmark_http(samples, || {
        client
            .get(format!("http://127.0.0.1:{port}/v1/search"))
            .query(&[("q", "benchmark"), ("limit", "20")])
            .send()
    })
}

fn benchmark_show(
    client: &BlockingClient,
    port: u16,
    locator: &str,
    samples: usize,
) -> Result<f64, String> {
    benchmark_http(samples, || {
        client
            .get(format!("http://127.0.0.1:{port}/v1/skills/show"))
            .query(&[("locator", locator)])
            .send()
    })
}

fn benchmark_horizontal_search(client: &BlockingClient, samples: usize) -> Result<f64, String> {
    let mut next = false;
    benchmark_http(samples, || {
        next = !next;
        let port = if next {
            SERVER_ONE_PORT
        } else {
            SERVER_TWO_PORT
        };
        client
            .get(format!("http://127.0.0.1:{port}/v1/search"))
            .query(&[("q", "benchmark"), ("limit", "20")])
            .send()
    })
}

fn benchmark_http<F>(samples: usize, mut request: F) -> Result<f64, String>
where
    F: FnMut() -> Result<reqwest::blocking::Response, reqwest::Error>,
{
    for _ in 0..10 {
        let response = request().map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP warmup returned {}", response.status()));
        }
    }
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let response = request().map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP benchmark returned {}", response.status()));
        }
        durations.push(started.elapsed());
    }
    Ok(p95_ms(&mut durations))
}

fn assert_show(client: &BlockingClient, port: u16, locator: &str) -> Result<(), String> {
    let response = client
        .get(format!("http://127.0.0.1:{port}/v1/skills/show"))
        .query(&[("locator", locator)])
        .send()
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("show after restart returned {}", response.status()))
    }
}

async fn benchmark_reconcile(
    registry: &Registry,
    seeded: &[SeededPublicSkill],
) -> Result<Vec<ReconcileReport>, String> {
    let mut reports = Vec::new();
    for watched in [25_usize, 100, 200] {
        let bearer = fixture_bearer(&format!("phase17-installation-{watched}"));
        create_installation(registry, &bearer).await?;
        for skill in seeded.iter().take(watched) {
            let operation = Uuid::now_v7().to_string();
            let request_hash = subscription_request_hash(
                SubscriptionMutationKind::Subscribe,
                &operation,
                &skill.resource_id,
                skill.generation,
                None,
                false,
            )
            .map_err(|error| error.to_string())?;
            registry
                .mutate_subscription(
                    &bearer,
                    SubscriptionMutationKind::Subscribe,
                    &SubscriptionMutationRequest {
                        operation_id: operation,
                        resource_id: skill.resource_id.clone(),
                        expected_generation: skill.generation,
                        release_version: None,
                        retain_on_delete: false,
                        request_hash: request_hash.to_string(),
                    },
                )
                .await
                .map_err(|error| error.message)?;
        }
        let request = SyncReconcileRequest {
            known: seeded
                .iter()
                .take(watched)
                .map(|skill| SyncKnownResource {
                    resource_id: skill.resource_id.clone(),
                    generation: skill.generation,
                    revision_id: skill.revision_id.clone(),
                })
                .collect(),
        };
        let mut durations = Vec::with_capacity(30);
        for _ in 0..30 {
            let started = Instant::now();
            let response = registry
                .reconcile_subscriptions(&bearer, &request)
                .await
                .map_err(|error| error.message)?;
            if !response.skills.is_empty()
                || !response.removed_resource_ids.is_empty()
                || !response.quarantined.is_empty()
            {
                return Err("current reconcile unexpectedly returned a delta".to_owned());
            }
            durations.push(started.elapsed());
        }
        reports.push(ReconcileReport {
            watched_resources: watched,
            p95_ms: p95_ms(&mut durations),
        });
    }
    Ok(reports)
}

fn benchmark_missed_event_reconcile(
    runtime: &Runtime,
    registry: &Registry,
    root: &Path,
    seeded: &[SeededPublicSkill],
) -> Result<MissedEventReconcileReport, String> {
    const WATCHED: usize = 100;
    const MISSED_EVENTS: usize = 4_000;
    let bearer = "4a91b7c36f8f7ddb8841857479fae9364e1d1ec9dff7eb2f9790ee625386e04c";
    runtime.block_on(create_installation(registry, bearer))?;
    for skill in seeded.iter().take(WATCHED) {
        runtime.block_on(subscribe_one(registry, bearer, skill))?;
    }
    let request = SyncReconcileRequest {
        known: seeded
            .iter()
            .take(WATCHED)
            .map(|skill| SyncKnownResource {
                resource_id: skill.resource_id.clone(),
                generation: skill.generation,
                revision_id: skill.revision_id.clone(),
            })
            .collect(),
    };
    let (before_p95_ms, before_payload_bytes) =
        runtime.block_on(measure_reconcile(registry, bearer, &request, 30))?;

    let resource = &seeded[0];
    docker_psql(
        root,
        DATABASE,
        &format!(
            "INSERT INTO authority_events(event_kind,resource_id,resource_generation,payload_json) \
             SELECT 'phase17_missed_history','{}'::uuid,{},'{{}}'::jsonb \
             FROM generate_series(1,{MISSED_EVENTS});",
            resource.resource_id, resource.generation
        ),
    )?;

    let (after_p95_ms, after_payload_bytes) =
        runtime.block_on(measure_reconcile(registry, bearer, &request, 30))?;
    if before_payload_bytes != after_payload_bytes {
        return Err(format!(
            "reconcile payload changed after missed-event history grew: {before_payload_bytes} -> {after_payload_bytes} bytes"
        ));
    }
    Ok(MissedEventReconcileReport {
        watched_resources: WATCHED,
        synthetic_missed_events: MISSED_EVENTS,
        before_p95_ms,
        after_p95_ms,
        before_payload_bytes,
        after_payload_bytes,
    })
}

async fn measure_reconcile(
    registry: &Registry,
    bearer: &str,
    request: &SyncReconcileRequest,
    samples: usize,
) -> Result<(f64, usize), String> {
    let mut durations = Vec::with_capacity(samples);
    let mut payload_bytes = None;
    for _ in 0..samples {
        let started = Instant::now();
        let response = registry
            .reconcile_subscriptions(bearer, request)
            .await
            .map_err(|error| error.message)?;
        durations.push(started.elapsed());
        if !response.skills.is_empty()
            || !response.removed_resource_ids.is_empty()
            || !response.quarantined.is_empty()
        {
            return Err("current reconcile unexpectedly returned a delta".to_owned());
        }
        let size = serde_json::to_vec(&response)
            .map_err(|error| error.to_string())?
            .len();
        match payload_bytes {
            Some(expected) if expected != size => {
                return Err("identical reconcile roots produced unstable payload size".to_owned());
            }
            Some(_) => {}
            None => payload_bytes = Some(size),
        }
    }
    Ok((
        p95_ms(&mut durations),
        payload_bytes.ok_or_else(|| "reconcile benchmark produced no samples".to_owned())?,
    ))
}

async fn create_installation(registry: &Registry, bearer: &str) -> Result<(), String> {
    let raw = hex::decode(bearer).map_err(|error| format!("invalid fixture bearer: {error}"))?;
    if raw.len() != 32 {
        return Err("fixture bearer must decode to 32 bytes".to_owned());
    }
    let credential_hash = format!("{:x}", Sha256::digest(&raw));
    let operation = Uuid::now_v7().to_string();
    let request_hash = create_installation_request_hash(&operation, &credential_hash)
        .map_err(|error| error.to_string())?;
    registry
        .create_installation(&CreateInstallationRequest {
            operation_id: operation,
            credential_hash,
            request_hash: request_hash.to_string(),
        })
        .await
        .map_err(|error| error.message)?;
    Ok(())
}

fn fixture_bearer(label: &str) -> String {
    format!("{:x}", Sha256::digest(label.as_bytes()))
}

fn exercise_sse_reconnect(
    runtime: &Runtime,
    root: &Path,
    server_binary: &Path,
    blocking: &BlockingClient,
    registry: &Registry,
    seeded: &[SeededPublicSkill],
    server_two: &mut ServerProcess,
) -> Result<(bool, bool), String> {
    let bearer = "84f6b1bdc132e2f5a2e60a68530f8472f836713c246bb2cb4c065c2eec26570b";
    runtime.block_on(create_installation(registry, bearer))?;
    runtime.block_on(subscribe_one(registry, bearer, &seeded[0]))?;
    let async_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .build()
        .map_err(|error| error.to_string())?;

    let mut first = runtime.block_on(async {
        async_client
            .get(format!("http://127.0.0.1:{SERVER_TWO_PORT}/v1/events"))
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|error| error.to_string())
    })?;
    if !first.status().is_success() {
        return Err(format!("initial SSE returned {}", first.status()));
    }
    server_two.kill()?;
    let disconnected = runtime.block_on(async {
        timeout(Duration::from_secs(2), async {
            loop {
                match first.chunk().await {
                    Ok(Some(_buffered_event)) => continue,
                    Ok(None) | Err(_) => return true,
                }
            }
        })
        .await
        .unwrap_or(false)
    });

    *server_two = ServerProcess::start(server_binary, SERVER_TWO_PORT)?;
    wait_ready(blocking, SERVER_TWO_PORT)?;
    let mut second = runtime.block_on(async {
        async_client
            .get(format!("http://127.0.0.1:{SERVER_TWO_PORT}/v1/events"))
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|error| error.to_string())
    })?;
    insert_recovery_outbox_event(root)?;
    drain_outbox(blocking, SERVER_ONE_PORT)?;
    let reconnected = runtime
        .block_on(async {
            timeout(Duration::from_secs(2), async {
                loop {
                    match second.chunk().await {
                        Ok(Some(bytes)) => {
                            if String::from_utf8_lossy(&bytes).contains("resync_all") {
                                return Ok(true);
                            }
                        }
                        Ok(None) => return Ok(false),
                        Err(error) => return Err(error.to_string()),
                    }
                }
            })
            .await
        })
        .map_err(|_| "reconnected SSE did not receive a wake within 2 seconds".to_owned())??;
    Ok((disconnected, reconnected))
}

async fn subscribe_one(
    registry: &Registry,
    bearer: &str,
    skill: &SeededPublicSkill,
) -> Result<(), String> {
    let operation = Uuid::now_v7().to_string();
    let request_hash = subscription_request_hash(
        SubscriptionMutationKind::Subscribe,
        &operation,
        &skill.resource_id,
        skill.generation,
        None,
        false,
    )
    .map_err(|error| error.to_string())?;
    registry
        .mutate_subscription(
            bearer,
            SubscriptionMutationKind::Subscribe,
            &SubscriptionMutationRequest {
                operation_id: operation,
                resource_id: skill.resource_id.clone(),
                expected_generation: skill.generation,
                release_version: None,
                retain_on_delete: false,
                request_hash: request_hash.to_string(),
            },
        )
        .await
        .map_err(|error| error.message)?;
    Ok(())
}

fn insert_recovery_outbox_event(root: &Path) -> Result<(), String> {
    docker_psql(
        root,
        DATABASE,
        "WITH e AS (INSERT INTO authority_events(event_kind,payload_json) VALUES ('phase17_recovery','{}'::jsonb) RETURNING id) INSERT INTO outbox_events(event_id,event_kind,payload_json) SELECT id,'resync_all','{}'::jsonb FROM e;",
    )
    .map(|_| ())
}

fn drain_outbox(client: &BlockingClient, port: u16) -> Result<usize, String> {
    let response = client
        .post(format!("http://127.0.0.1:{port}/v1/internal/outbox/drain"))
        .bearer_auth(RECOVERY_TOKEN)
        .json(&serde_json::json!({"limit": 256}))
        .send()
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("outbox recovery returned {}", response.status()));
    }
    let value: Value = response.json().map_err(|error| error.to_string())?;
    value["dispatched"]
        .as_u64()
        .map(|value| value as usize)
        .ok_or_else(|| "outbox recovery response omitted dispatched count".to_owned())
}

fn pending_outbox(root: &Path) -> Result<u64, String> {
    let output = docker_psql(
        root,
        DATABASE,
        "SELECT COUNT(*) FROM outbox_events WHERE dispatched_at IS NULL;",
    )?;
    output
        .lines()
        .find_map(|line| line.trim().parse::<u64>().ok())
        .ok_or_else(|| "could not parse pending outbox count".to_owned())
}

fn exercise_listener_reconnect(
    runtime: &Runtime,
    root: &Path,
    observer: &Registry,
) -> Result<bool, String> {
    let before = runtime
        .block_on(observer.operational_metrics())
        .map_err(|error| error.to_string())?;
    let before_connections = before.process.wake_listener_connections_total;
    let terminated = docker_psql(
        root,
        DATABASE,
        "WITH candidates AS (SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND usename='denju_app' AND pid<>pg_backend_pid() AND query ILIKE 'LISTEN%denju_wake%') SELECT COUNT(*) FROM candidates WHERE pg_terminate_backend(pid);",
    )?;
    let terminated_count = terminated
        .lines()
        .find_map(|line| line.trim().parse::<u64>().ok())
        .ok_or_else(|| "could not parse terminated LISTEN connection count".to_owned())?;
    if terminated_count == 0 {
        return Err("listener reconnect fixture did not terminate a LISTEN backend".to_owned());
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let snapshot = runtime
            .block_on(observer.operational_metrics())
            .map_err(|error| error.to_string())?;
        if snapshot.process.wake_listener_connected
            && snapshot.process.wake_listener_connections_total > before_connections
        {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(false)
}

fn explain_search(root: &Path) -> Result<String, String> {
    docker_psql(
        root,
        DATABASE,
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) SELECT sd.resource_id FROM resource_search_documents sd JOIN resources r ON r.id=sd.resource_id WHERE r.deleted_at IS NULL AND r.visibility='public' AND sd.search_vector @@ websearch_to_tsquery('simple','benchmark') ORDER BY sd.star_count DESC,sd.owner_slug,sd.resource_slug LIMIT 20;",
    )
}

fn explain_outbox(root: &Path) -> Result<String, String> {
    docker_psql(
        root,
        DATABASE,
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) SELECT event_id,event_kind,payload_json FROM outbox_events WHERE dispatched_at IS NULL ORDER BY event_id LIMIT 256;",
    )
}

fn enforce_targets(report: &LoadReport) -> Result<(), String> {
    if report.registry_search_p95_ms >= 200.0
        || report.registry_show_p95_ms >= 200.0
        || report.single_instance_concurrent_search_p95_ms >= 200.0
    {
        return Err(format!(
            "registry p95 target missed: search {:.2}ms, show {:.2}ms, concurrent search {:.2}ms",
            report.registry_search_p95_ms,
            report.registry_show_p95_ms,
            report.single_instance_concurrent_search_p95_ms
        ));
    }
    if report.cli_status_latency.p50_ms >= 50.0 || report.cli_search_latency.p50_ms >= 50.0 {
        return Err(format!(
            "normal-use CLI target missed: status p50 {:.2}ms (p95 {:.2}ms), search p50 {:.2}ms (p95 {:.2}ms)",
            report.cli_status_latency.p50_ms,
            report.cli_status_latency.p95_ms,
            report.cli_search_latency.p50_ms,
            report.cli_search_latency.p95_ms
        ));
    }
    if report.cross_instance_wake_ms >= 2_000 {
        return Err(format!(
            "cross-instance wake target missed: {}ms",
            report.cross_instance_wake_ms
        ));
    }
    if !report.sse_disconnect_observed || !report.sse_reconnect_observed {
        return Err("SSE disconnect/reconnect lifecycle was not fully observed".to_owned());
    }
    Ok(())
}

fn write_report(root: &Path, report: &LoadReport) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    let path = root.join("tmp/gg/denju-rust-greenfield/phase17-load-report-latest.json");
    fs::write(&path, format!("{json}\n")).map_err(|error| error.to_string())?;
    println!("load report: {}", path.display());
    Ok(())
}
