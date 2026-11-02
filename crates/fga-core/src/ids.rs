use base64::{Engine, engine::general_purpose::STANDARD};

/// Opaque, stable IDs; no local database or GitHub numeric IDs are involved.
pub fn node_id(kind: &str, repository: &str, id: impl std::fmt::Display) -> String {
    STANDARD.encode(format!("fga:{kind}:{repository}:{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_includes_kind_repository_and_number() {
        assert_eq!(
            STANDARD
                .decode(node_id("PullRequest", "e2e/oracle", 1))
                .unwrap(),
            b"fga:PullRequest:e2e/oracle:1"
        );
        assert_ne!(
            node_id("PullRequest", "a/r", 1),
            node_id("PullRequest", "b/r", 1)
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NodeId {
    pub kind: String,
    pub owner: String,
    pub repository: String,
    pub number: i32,
}

pub fn decode(value: &str) -> Option<NodeId> {
    let bytes = STANDARD.decode(value).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut parts = text.split(':');
    if parts.next()? != "fga" {
        return None;
    }
    let kind = parts.next()?.to_owned();
    let (owner, repository) = parts.next()?.split_once('/')?;
    if owner.is_empty() || repository.is_empty() || repository.contains('/') {
        return None;
    }
    let number = parts.next()?.parse().ok().filter(|n: &i32| *n > 0)?;
    if parts.next().is_some() {
        return None;
    }
    Some(NodeId {
        kind,
        owner: owner.into(),
        repository: repository.into(),
        number,
    })
}

#[cfg(test)]
mod decode_tests {
    use super::*;
    #[test]
    fn round_trip_and_malformed_ids() {
        assert_eq!(
            decode(&node_id("PullRequest", "o/r", 42)),
            Some(NodeId {
                kind: "PullRequest".into(),
                owner: "o".into(),
                repository: "r".into(),
                number: 42
            })
        );
        for value in [
            "garbage".into(),
            STANDARD.encode("other:PullRequest:o/r:1"),
            node_id("PullRequest", "o/r", -1),
            node_id("PullRequest", "o/r/x", 1),
            STANDARD.encode("fga:PullRequest:o/r:1:extra"),
        ] {
            assert_eq!(decode(&value), None);
        }
    }
}
