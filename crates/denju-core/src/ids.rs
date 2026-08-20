use std::{fmt, str::FromStr};

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum IdError {
    #[error("invalid UUID: {0}")]
    InvalidUuid(uuid::Error),
    #[error("Denju mutable IDs must be UUIDv7")]
    NotUuidV7,
}

macro_rules! uuid_v7_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            pub fn from_uuid(value: Uuid) -> Result<Self, IdError> {
                if value.get_version_num() == 7 {
                    Ok(Self(value))
                } else {
                    Err(IdError::NotUuidV7)
                }
            }

            pub const fn as_uuid(self) -> Uuid {
                self.0
            }

            pub const fn as_bytes(&self) -> &[u8; 16] {
                self.0.as_bytes()
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let uuid = Uuid::parse_str(value).map_err(IdError::InvalidUuid)?;
                Self::from_uuid(uuid)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

uuid_v7_id!(ResourceId);
uuid_v7_id!(NamespaceId);
uuid_v7_id!(AuthorPrincipalId);
uuid_v7_id!(OperationId);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_require_uuid_v7() {
        let valid = "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1"
            .parse::<ResourceId>()
            .expect("valid UUIDv7");
        assert_eq!(valid.to_string(), "01890f47-6a1c-7cc2-98c1-5f6c1ed8a3a1");

        let invalid = "550e8400-e29b-41d4-a716-446655440000".parse::<ResourceId>();
        assert!(matches!(invalid, Err(IdError::NotUuidV7)));
    }

    #[test]
    fn generations_advance_without_wrapping() {
        assert_eq!(Generation::ZERO.checked_next(), Some(Generation::new(1)));
        assert_eq!(Generation::new(u64::MAX).checked_next(), None);
    }
}
