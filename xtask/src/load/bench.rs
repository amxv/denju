use std::{
    sync::{Arc, Barrier, Mutex},
    thread,
    time::Instant,
};

use reqwest::blocking::Client as BlockingClient;

use super::support::{ServerProcess, millis, p95_ms, wait_ready};

pub(super) fn repeated_cold_starts(
    binary: &std::path::Path,
    client: &BlockingClient,
    port: u16,
    samples: usize,
) -> Result<Vec<u64>, String> {
    let mut starts = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let mut server = ServerProcess::start(binary, port)?;
        wait_ready(client, port)?;
        starts.push(millis(started.elapsed()));
        server.terminate()?;
    }
    Ok(starts)
}

pub(super) fn concurrent_search_p95(
    client: &BlockingClient,
    port: u16,
    workers: usize,
    requests_per_worker: usize,
) -> Result<f64, String> {
    let barrier = Arc::new(Barrier::new(workers));
    let durations = Arc::new(Mutex::new(Vec::with_capacity(
        workers.saturating_mul(requests_per_worker),
    )));
    let mut joins = Vec::with_capacity(workers);
    for _ in 0..workers {
        let client = client.clone();
        let barrier = barrier.clone();
        let durations = durations.clone();
        joins.push(thread::spawn(move || -> Result<(), String> {
            barrier.wait();
            for _ in 0..requests_per_worker {
                let started = Instant::now();
                let response = client
                    .get(format!("http://127.0.0.1:{port}/v1/search"))
                    .query(&[("q", "benchmark"), ("limit", "20")])
                    .send()
                    .map_err(|error| error.to_string())?;
                if !response.status().is_success() {
                    return Err(format!("concurrent search returned {}", response.status()));
                }
                durations
                    .lock()
                    .map_err(|_| "concurrent benchmark mutex was poisoned".to_owned())?
                    .push(started.elapsed());
            }
            Ok(())
        }));
    }
    for join in joins {
        join.join()
            .map_err(|_| "concurrent benchmark worker panicked".to_owned())??;
    }
    let mut durations = Arc::try_unwrap(durations)
        .map_err(|_| "concurrent benchmark durations still shared".to_owned())?
        .into_inner()
        .map_err(|_| "concurrent benchmark mutex was poisoned".to_owned())?;
    if durations.is_empty() {
        return Err("concurrent benchmark produced no samples".to_owned());
    }
    Ok(p95_ms(&mut durations))
}

pub(super) fn max_ms(values: &[u64]) -> Result<u64, String> {
    values
        .iter()
        .copied()
        .max()
        .ok_or_else(|| "benchmark produced no samples".to_owned())
}
