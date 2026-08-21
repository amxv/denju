use std::collections::BTreeSet;

use tokio::sync::broadcast;

use super::*;

#[tokio::test]
async fn sync_hints_coalesce_duplicate_resources_and_filter_unwatched_resources() {
    let (sender, mut receiver) = broadcast::channel(16);
    let watched_id = uuid::Uuid::now_v7();
    let unrelated_id = uuid::Uuid::now_v7();
    let watched = BTreeSet::from([watched_id]);
    sender
        .send(RegistryWake::Resource {
            resource_id: watched_id,
            generation: 2,
        })
        .unwrap();
    sender
        .send(RegistryWake::Resource {
            resource_id: unrelated_id,
            generation: 99,
        })
        .unwrap();
    sender
        .send(RegistryWake::Resource {
            resource_id: watched_id,
            generation: 4,
        })
        .unwrap();

    let hint = next_sync_hint(&mut receiver, &watched).await.unwrap();
    assert_eq!(
        hint,
        SyncHint::Dirty {
            resources: vec![denju_wire::DirtyResource {
                resource_id: watched_id.to_string(),
                generation: 4,
            }],
        }
    );
}

#[tokio::test]
async fn sync_hint_overflow_degrades_to_resync_all() {
    let (sender, mut receiver) = broadcast::channel(128);
    let watched = (0..65)
        .map(|_| uuid::Uuid::now_v7())
        .collect::<BTreeSet<_>>();
    for resource_id in &watched {
        sender
            .send(RegistryWake::Resource {
                resource_id: *resource_id,
                generation: 1,
            })
            .unwrap();
    }

    assert_eq!(
        next_sync_hint(&mut receiver, &watched).await,
        Some(SyncHint::ResyncAll)
    );
}
