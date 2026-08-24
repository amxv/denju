use std::{collections::BTreeMap, convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Extension,
    extract::State,
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
};
use denju_registry::{Registry, RegistryWake};
use denju_wire::SyncHint;
use futures_util::stream;

use crate::observability::HttpMetrics;

use super::{ApiResponseError, auth::bearer_token};

pub(super) async fn events(
    State(registry): State<Arc<Registry>>,
    Extension(metrics): Extension<Arc<HttpMetrics>>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, ApiResponseError> {
    let bearer = bearer_token(&headers)?;
    registry.ensure_wake_listener();
    let watched = registry
        .watched_resource_ids(bearer)
        .await
        .map_err(ApiResponseError)?;
    let receiver = registry.subscribe_wakes();
    let _ = registry.drain_outbox(256).await;
    let guard = metrics.start_sse();
    let event_stream = stream::unfold(
        (receiver, watched, guard, metrics),
        |(mut receiver, watched, guard, metrics)| async move {
            loop {
                match next_sync_hint_batch(&mut receiver, &watched).await {
                    Some(batch) => {
                        if batch.overflowed {
                            metrics.record_sse_overflow();
                        }
                        let event = Event::default()
                            .event("sync")
                            .json_data(batch.hint)
                            .unwrap_or_else(|_| {
                                Event::default()
                                    .event("sync")
                                    .data("{\"kind\":\"resync_all\"}")
                            });
                        return Some((Ok(event), (receiver, watched, guard, metrics)));
                    }
                    None if receiver.is_closed() => return None,
                    None => continue,
                }
            }
        },
    );
    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

struct SyncHintBatch {
    hint: SyncHint,
    overflowed: bool,
}

#[cfg(test)]
pub(crate) async fn next_sync_hint(
    receiver: &mut tokio::sync::broadcast::Receiver<RegistryWake>,
    watched: &std::collections::BTreeSet<uuid::Uuid>,
) -> Option<SyncHint> {
    next_sync_hint_batch(receiver, watched)
        .await
        .map(|batch| batch.hint)
}

async fn next_sync_hint_batch(
    receiver: &mut tokio::sync::broadcast::Receiver<RegistryWake>,
    watched: &std::collections::BTreeSet<uuid::Uuid>,
) -> Option<SyncHintBatch> {
    const MAX_DIRTY: usize = 64;
    let first = match receiver.recv().await {
        Ok(wake) => wake,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            return Some(SyncHintBatch {
                hint: SyncHint::ResyncAll,
                overflowed: true,
            });
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
    };
    if matches!(first, RegistryWake::ResyncAll) {
        return Some(SyncHintBatch {
            hint: SyncHint::ResyncAll,
            overflowed: false,
        });
    }
    let mut dirty = BTreeMap::<uuid::Uuid, u64>::new();
    if let RegistryWake::Resource {
        resource_id,
        generation,
    } = first
        && watched.contains(&resource_id)
    {
        dirty.insert(resource_id, generation);
    }
    loop {
        match receiver.try_recv() {
            Ok(RegistryWake::ResyncAll) => {
                return Some(SyncHintBatch {
                    hint: SyncHint::ResyncAll,
                    overflowed: false,
                });
            }
            Ok(RegistryWake::Resource {
                resource_id,
                generation,
            }) => {
                if watched.contains(&resource_id) {
                    dirty
                        .entry(resource_id)
                        .and_modify(|current| *current = (*current).max(generation))
                        .or_insert(generation);
                    if dirty.len() > MAX_DIRTY {
                        return Some(SyncHintBatch {
                            hint: SyncHint::ResyncAll,
                            overflowed: true,
                        });
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                return Some(SyncHintBatch {
                    hint: SyncHint::ResyncAll,
                    overflowed: true,
                });
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }
    if dirty.is_empty() {
        None
    } else {
        Some(SyncHintBatch {
            hint: SyncHint::Dirty {
                resources: dirty
                    .into_iter()
                    .map(|(resource_id, generation)| denju_wire::DirtyResource {
                        resource_id: resource_id.to_string(),
                        generation,
                    })
                    .collect(),
            },
            overflowed: false,
        })
    }
}
