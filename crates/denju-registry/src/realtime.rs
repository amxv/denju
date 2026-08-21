use denju_wire::{DirtyResource, SyncHint};

use crate::RegistryWake;

pub(crate) fn wake_as_sync_hint(wake: &RegistryWake) -> SyncHint {
    match wake {
        RegistryWake::Resource {
            resource_id,
            generation,
        } => SyncHint::Dirty {
            resources: vec![DirtyResource {
                resource_id: resource_id.to_string(),
                generation: *generation,
            }],
        },
        RegistryWake::ResyncAll => SyncHint::ResyncAll,
    }
}
