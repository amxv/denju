use std::{fmt, str::FromStr};

use thiserror::Error;

use crate::{PortablePath, validate_skill_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Skill,
    Pack,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceLocator {
    owner: String,
    name: String,
    kind: ResourceKind,
}

impl ResourceLocator {
    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }
}

impl fmt::Display for ResourceLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ResourceKind::Skill => write!(formatter, "@{}/{}", self.owner, self.name),
            ResourceKind::Pack => write!(formatter, "@{}/packs/{}", self.owner, self.name),
        }
    }
}

impl FromStr for ResourceLocator {
    type Err = LocatorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let rest = value.strip_prefix('@').ok_or(LocatorError::MissingAtSign)?;
        let segments = rest.split('/').collect::<Vec<_>>();
        let (owner, kind, name) = match segments.as_slice() {
            [owner, name] => (*owner, ResourceKind::Skill, *name),
            [owner, "packs", name] => (*owner, ResourceKind::Pack, *name),
            _ => return Err(LocatorError::InvalidShape),
        };

        validate_namespace_segment(owner)?;
        match kind {
            ResourceKind::Skill => {
                validate_skill_name(name).map_err(|_| LocatorError::InvalidResourceName)?;
            }
            ResourceKind::Pack => validate_pack_name(name)?,
        }

        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
            kind,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LocatorError {
    #[error("resource locators must start with '@'")]
    MissingAtSign,
    #[error("resource locators must be @owner/name or @owner/packs/name")]
    InvalidShape,
    #[error("namespace names must be non-empty lowercase path-safe text")]
    InvalidNamespace,
    #[error("invalid resource name")]
    InvalidResourceName,
}

fn validate_namespace_segment(value: &str) -> Result<(), LocatorError> {
    if !is_lowercase_portable_segment(value) {
        return Err(LocatorError::InvalidNamespace);
    }
    Ok(())
}

fn validate_pack_name(value: &str) -> Result<(), LocatorError> {
    if !is_lowercase_portable_segment(value) {
        return Err(LocatorError::InvalidResourceName);
    }
    Ok(())
}

fn is_lowercase_portable_segment(value: &str) -> bool {
    !value.contains('@')
        && !value.chars().any(char::is_uppercase)
        && PortablePath::parse(value).is_ok_and(|path| path.component_count() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_and_pack_locators() {
        let skill = "@alice/code-review"
            .parse::<ResourceLocator>()
            .expect("skill locator");
        assert_eq!(skill.kind(), ResourceKind::Skill);
        assert_eq!(skill.to_string(), "@alice/code-review");

        let pack = "@acme/packs/core"
            .parse::<ResourceLocator>()
            .expect("pack locator");
        assert_eq!(pack.kind(), ResourceKind::Pack);
        assert_eq!(pack.to_string(), "@acme/packs/core");
    }

    #[test]
    fn rejects_ambiguous_locator_shapes() {
        assert_eq!(
            "alice/code-review".parse::<ResourceLocator>(),
            Err(LocatorError::MissingAtSign)
        );
        assert_eq!(
            "@alice/skills/code-review".parse::<ResourceLocator>(),
            Err(LocatorError::InvalidShape)
        );
        assert_eq!(
            "@Alice/code-review".parse::<ResourceLocator>(),
            Err(LocatorError::InvalidNamespace)
        );
        assert_eq!(
            "@alice:dev/code-review".parse::<ResourceLocator>(),
            Err(LocatorError::InvalidNamespace)
        );
    }
}
