use std::collections::BTreeSet;

use denju_registry::RegistryWake;
use denju_wire::SyncHint;
use tokio::sync::broadcast;

use crate::{
    http::realtime_routes::next_sync_hint, parse_http_url, parse_http_url_with_http,
    public_origin_from,
};

#[test]
fn hosted_service_urls_require_tls_but_loopback_development_stays_available() {
    assert!(parse_http_url("DENJU_PUBLIC_URL", "https://registry.example.com").is_ok());
    assert!(parse_http_url("DENJU_S3_ENDPOINT", "http://127.0.0.1:53900").is_ok());
    assert!(parse_http_url("DENJU_S3_ENDPOINT", "http://[::1]:53900").is_ok());
    assert!(parse_http_url("DENJU_PUBLIC_URL", "http://registry.example.com").is_err());
    assert!(parse_http_url("DENJU_S3_ENDPOINT", "http://garage:3900").is_err());
    assert!(parse_http_url_with_http("DENJU_S3_ENDPOINT", "http://garage:3900", true).is_ok());
    assert!(
        parse_http_url(
            "DENJU_S3_ENDPOINT",
            "https://user:secret@objects.example.com"
        )
        .is_err()
    );
}

#[test]
fn explicit_public_origin_wins_and_vercel_preview_origin_is_derived_safely() {
    assert_eq!(
        public_origin_from(
            Some("https://registry.denju.ashray.xyz"),
            Some("preview.example.vercel.app")
        )
        .unwrap()
        .as_str(),
        "https://registry.denju.ashray.xyz/"
    );
    assert_eq!(
        public_origin_from(None, Some("preview.example.vercel.app"))
            .unwrap()
            .as_str(),
        "https://preview.example.vercel.app/"
    );
    assert!(public_origin_from(None, Some("http://bad.example.com")).is_err());
    assert!(public_origin_from(None, None).is_err());
}

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
