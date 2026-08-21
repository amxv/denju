use denju_registry::Registry;

use super::ServerConfig;

pub(super) async fn gc(config: &ServerConfig, limit: u32) -> Result<(), String> {
    let registry = Registry::connect(config.registry_settings())
        .await
        .map_err(|error| error.to_string())?;
    registry
        .validate_schema()
        .await
        .map_err(|error| error.to_string())?;
    let deleted = registry
        .drain_blob_gc(limit)
        .await
        .map_err(|error| error.message)?;
    println!("deleted {deleted} canonical blobs");
    Ok(())
}
