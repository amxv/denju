use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{RequestHash, RequestHashError};

const PROFILE_UPDATE_DOMAIN: &[u8] = b"denju:http:v1:profile-update\0";
const FOLLOW_DOMAIN: &[u8] = b"denju:http:v1:follow\0";
const UNFOLLOW_DOMAIN: &[u8] = b"denju:http:v1:unfollow\0";
const STAR_DOMAIN: &[u8] = b"denju:http:v1:star\0";
const UNSTAR_DOMAIN: &[u8] = b"denju:http:v1:unstar\0";
const TOPICS_DOMAIN: &[u8] = b"denju:http:v1:resource-topics\0";
const REPORT_DOMAIN: &[u8] = b"denju:http:v1:report-resource\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowMutationKind {
    Follow,
    Unfollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarMutationKind {
    Star,
    Unstar,
}

fn hash<T: Serialize>(domain: &[u8], value: &T) -> Result<RequestHash, RequestHashError> {
    let canonical = serde_json_canonicalizer::to_vec(value)
        .map_err(|error| RequestHashError::Canonicalization(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(canonical);
    Ok(RequestHash::from_bytes(hasher.finalize().into()))
}

#[derive(Serialize)]
struct ProfileUpdateHashInput<'a> {
    operation_id: &'a str,
    bio: Option<&'a str>,
    followers_visible: bool,
    following_visible: bool,
}

pub fn profile_update_request_hash(
    operation_id: &str,
    bio: Option<&str>,
    followers_visible: bool,
    following_visible: bool,
) -> Result<RequestHash, RequestHashError> {
    hash(
        PROFILE_UPDATE_DOMAIN,
        &ProfileUpdateHashInput {
            operation_id,
            bio,
            followers_visible,
            following_visible,
        },
    )
}

#[derive(Serialize)]
struct TargetHashInput<'a> {
    operation_id: &'a str,
    target_id: &'a str,
}

pub fn follow_request_hash(
    kind: FollowMutationKind,
    operation_id: &str,
    target_user_id: &str,
) -> Result<RequestHash, RequestHashError> {
    hash(
        match kind {
            FollowMutationKind::Follow => FOLLOW_DOMAIN,
            FollowMutationKind::Unfollow => UNFOLLOW_DOMAIN,
        },
        &TargetHashInput {
            operation_id,
            target_id: target_user_id,
        },
    )
}

pub fn star_request_hash(
    kind: StarMutationKind,
    operation_id: &str,
    resource_id: &str,
) -> Result<RequestHash, RequestHashError> {
    hash(
        match kind {
            StarMutationKind::Star => STAR_DOMAIN,
            StarMutationKind::Unstar => UNSTAR_DOMAIN,
        },
        &TargetHashInput {
            operation_id,
            target_id: resource_id,
        },
    )
}

#[derive(Serialize)]
struct TopicsHashInput<'a> {
    operation_id: &'a str,
    resource_id: &'a str,
    expected_generation: u64,
    topics: &'a [String],
}

pub fn resource_topics_request_hash(
    operation_id: &str,
    resource_id: &str,
    expected_generation: u64,
    topics: &[String],
) -> Result<RequestHash, RequestHashError> {
    hash(
        TOPICS_DOMAIN,
        &TopicsHashInput {
            operation_id,
            resource_id,
            expected_generation,
            topics,
        },
    )
}

#[derive(Serialize)]
struct ReportHashInput<'a> {
    operation_id: &'a str,
    resource_id: &'a str,
    reason: &'a str,
}

pub fn report_resource_request_hash(
    operation_id: &str,
    resource_id: &str,
    reason: &str,
) -> Result<RequestHash, RequestHashError> {
    hash(
        REPORT_DOMAIN,
        &ReportHashInput {
            operation_id,
            resource_id,
            reason,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn social_mutation_domains_and_payloads_are_hash_bound() {
        let op = "01890f47-6a1d-7ad0-8f43-9a4d8c29f001";
        let target = "01890f47-6a1d-7ad0-8f43-9a4d8c29f002";
        assert_ne!(
            follow_request_hash(FollowMutationKind::Follow, op, target).unwrap(),
            follow_request_hash(FollowMutationKind::Unfollow, op, target).unwrap()
        );
        assert_ne!(
            star_request_hash(StarMutationKind::Star, op, target).unwrap(),
            star_request_hash(StarMutationKind::Unstar, op, target).unwrap()
        );
        assert_ne!(
            resource_topics_request_hash(op, target, 3, &["rust".into()]).unwrap(),
            resource_topics_request_hash(op, target, 4, &["rust".into()]).unwrap()
        );
        assert_ne!(
            report_resource_request_hash(op, target, "malicious").unwrap(),
            report_resource_request_hash(op, target, "spam").unwrap()
        );
    }
}
