use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;

use crate::{Registry, RegistryError};

static DATABASE_ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);
static OBJECT_STORE_ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);
static OBJECT_STORE_READ_BYTES_TOTAL: AtomicU64 = AtomicU64::new(0);
static OBJECT_STORE_WRITE_BYTES_TOTAL: AtomicU64 = AtomicU64::new(0);
static OUTBOX_DRAINS_TOTAL: AtomicU64 = AtomicU64::new(0);
static OUTBOX_EVENTS_DISPATCHED_TOTAL: AtomicU64 = AtomicU64::new(0);
static RECONCILE_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static RECONCILE_KNOWN_ROOTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static RECONCILE_CHANGED_ROOTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static WAKE_LISTENER_CONNECTIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static WAKE_LISTENER_ERRORS_TOTAL: AtomicU64 = AtomicU64::new(0);
static WAKE_NOTIFICATIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static WAKE_LISTENER_CONNECTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub struct RegistryMetricsSnapshot {
    pub database_errors_total: u64,
    pub object_store_errors_total: u64,
    pub object_store_read_bytes_total: u64,
    pub object_store_write_bytes_total: u64,
    pub outbox_drains_total: u64,
    pub outbox_events_dispatched_total: u64,
    pub reconcile_requests_total: u64,
    pub reconcile_known_roots_total: u64,
    pub reconcile_changed_roots_total: u64,
    pub wake_listener_connections_total: u64,
    pub wake_listener_errors_total: u64,
    pub wake_notifications_total: u64,
    pub wake_listener_connected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryOperationalMetrics {
    pub process: RegistryMetricsSnapshot,
    pub pending_outbox_events: u64,
    pub oldest_pending_outbox_age_ms: u64,
}

pub(crate) fn record_database_error() {
    DATABASE_ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_object_store_error() {
    OBJECT_STORE_ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_object_store_read_bytes(bytes: usize) {
    OBJECT_STORE_READ_BYTES_TOTAL.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub(crate) fn record_object_store_write_bytes(bytes: usize) {
    OBJECT_STORE_WRITE_BYTES_TOTAL.fetch_add(bytes as u64, Ordering::Relaxed);
}

pub(crate) fn record_outbox_drain() {
    OUTBOX_DRAINS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_outbox_dispatched(count: usize) {
    OUTBOX_EVENTS_DISPATCHED_TOTAL.fetch_add(count as u64, Ordering::Relaxed);
}

pub(crate) fn record_reconcile(known_roots: usize, changed_roots: usize) {
    RECONCILE_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    RECONCILE_KNOWN_ROOTS_TOTAL.fetch_add(known_roots as u64, Ordering::Relaxed);
    RECONCILE_CHANGED_ROOTS_TOTAL.fetch_add(changed_roots as u64, Ordering::Relaxed);
}

pub(crate) fn record_wake_listener_connected() {
    WAKE_LISTENER_CONNECTIONS_TOTAL.fetch_add(1, Ordering::Relaxed);
    WAKE_LISTENER_CONNECTED.store(true, Ordering::Release);
}

pub(crate) fn record_wake_listener_error() {
    WAKE_LISTENER_ERRORS_TOTAL.fetch_add(1, Ordering::Relaxed);
    WAKE_LISTENER_CONNECTED.store(false, Ordering::Release);
}

pub(crate) fn record_wake_notification() {
    WAKE_NOTIFICATIONS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

fn snapshot() -> RegistryMetricsSnapshot {
    RegistryMetricsSnapshot {
        database_errors_total: DATABASE_ERRORS_TOTAL.load(Ordering::Relaxed),
        object_store_errors_total: OBJECT_STORE_ERRORS_TOTAL.load(Ordering::Relaxed),
        object_store_read_bytes_total: OBJECT_STORE_READ_BYTES_TOTAL.load(Ordering::Relaxed),
        object_store_write_bytes_total: OBJECT_STORE_WRITE_BYTES_TOTAL.load(Ordering::Relaxed),
        outbox_drains_total: OUTBOX_DRAINS_TOTAL.load(Ordering::Relaxed),
        outbox_events_dispatched_total: OUTBOX_EVENTS_DISPATCHED_TOTAL.load(Ordering::Relaxed),
        reconcile_requests_total: RECONCILE_REQUESTS_TOTAL.load(Ordering::Relaxed),
        reconcile_known_roots_total: RECONCILE_KNOWN_ROOTS_TOTAL.load(Ordering::Relaxed),
        reconcile_changed_roots_total: RECONCILE_CHANGED_ROOTS_TOTAL.load(Ordering::Relaxed),
        wake_listener_connections_total: WAKE_LISTENER_CONNECTIONS_TOTAL.load(Ordering::Relaxed),
        wake_listener_errors_total: WAKE_LISTENER_ERRORS_TOTAL.load(Ordering::Relaxed),
        wake_notifications_total: WAKE_NOTIFICATIONS_TOTAL.load(Ordering::Relaxed),
        wake_listener_connected: WAKE_LISTENER_CONNECTED.load(Ordering::Acquire),
    }
}

impl Registry {
    pub async fn operational_metrics(&self) -> Result<RegistryOperationalMetrics, RegistryError> {
        let row = sqlx::query_as::<_, (i64, i64)>(
            "SELECT COUNT(*)::BIGINT, \
             COALESCE((EXTRACT(EPOCH FROM (now()-MIN(created_at)))::double precision * 1000)::BIGINT,0) \
             FROM outbox_events WHERE dispatched_at IS NULL",
        )
        .fetch_one(&self.worker_pool)
        .await
        .map_err(|error| {
            record_database_error();
            RegistryError::Database(error)
        })?;
        Ok(RegistryOperationalMetrics {
            process: snapshot(),
            pending_outbox_events: u64::try_from(row.0).unwrap_or(u64::MAX),
            oldest_pending_outbox_age_ms: u64::try_from(row.1).unwrap_or(0),
        })
    }
}
