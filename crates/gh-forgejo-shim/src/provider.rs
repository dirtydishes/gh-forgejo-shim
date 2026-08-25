//! Canonical Forgejo host profiles and transport-alias resolution.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::{is_known_github_host, normalize_host};
use crate::repo::RepoRef;
use crate::{Result, ShimError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProfile {
    pub canonical_host: String,
    pub aliases: Vec<String>,
    pub api_root: String,
    pub credential_host: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRegistry {
    profiles: Vec<HostProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderResolution {
    Forgejo { repo: RepoRef, profile: HostProfile },
    GitHub { repo: RepoRef },
    Unconfigured { repo: RepoRef },
}

impl HostRegistry {
    pub fn new(mut profiles: Vec<HostProfile>) -> Result<Self> {
        let mut assigned = BTreeMap::<String, usize>::new();
        for (profile_index, profile) in profiles.iter_mut().enumerate() {
            profile.canonical_host = normalize_host(&profile.canonical_host);
            if is_known_github_host(Some(&profile.canonical_host)) {
                return Err(ShimError::new(format!(
                    "GitHub host cannot be registered as Forgejo: {}",
                    profile.canonical_host
                )));
            }
            let api_root = reqwest::Url::parse(&profile.api_root)
                .map_err(|_| ShimError::new(format!("invalid API root: {}", profile.api_root)))?;
            if !matches!(api_root.scheme(), "http" | "https") {
                return Err(ShimError::new(format!(
                    "API root must use HTTP or HTTPS: {}",
                    profile.api_root
                )));
            }
            profile.aliases = profile
                .aliases
                .iter()
                .map(|alias| normalize_host(alias))
                .collect();

            let mut local = BTreeSet::new();
            for transport_host in std::iter::once(&profile.canonical_host).chain(&profile.aliases) {
                if !local.insert(transport_host.clone()) {
                    return Err(ShimError::new(format!(
                        "duplicate transport host: {transport_host}"
                    )));
                }
                if assigned
                    .insert(transport_host.clone(), profile_index)
                    .is_some()
                {
                    return Err(ShimError::new(format!(
                        "ambiguous transport host: {transport_host}"
                    )));
                }
            }
        }
        Ok(Self { profiles })
    }

    pub fn profiles(&self) -> &[HostProfile] {
        &self.profiles
    }

    pub fn resolve(&self, repo: &RepoRef) -> ProviderResolution {
        let transport_host = normalize_host(&repo.host);
        if is_known_github_host(Some(&transport_host)) {
            return ProviderResolution::GitHub { repo: repo.clone() };
        }

        if let Some(profile) = self.profiles.iter().find(|profile| {
            normalize_host(&profile.canonical_host) == transport_host
                || profile
                    .aliases
                    .iter()
                    .any(|alias| normalize_host(alias) == transport_host)
        }) {
            return ProviderResolution::Forgejo {
                repo: RepoRef::new(&profile.canonical_host, &repo.owner, &repo.name),
                profile: profile.clone(),
            };
        }

        ProviderResolution::Unconfigured { repo: repo.clone() }
    }
}

#[cfg(test)]
mod tests {
    use crate::repo::RepoRef;
    use crate::Result;

    use super::{HostProfile, HostRegistry, ProviderResolution};

    #[test]
    fn registry_resolves_transport_aliases_to_the_canonical_profile() -> Result<()> {
        let registry = HostRegistry::new(vec![HostProfile {
            canonical_host: "git.dirtydishes.dev".to_string(),
            aliases: vec!["127.0.0.1".to_string(), "127.0.0.1:2222".to_string()],
            api_root: "https://git.dirtydishes.dev/api/v1".to_string(),
            credential_host: "git.dirtydishes.dev".to_string(),
        }])?;

        for alias in ["127.0.0.1", "127.0.0.1:2222"] {
            let resolution = registry.resolve(&RepoRef::new(alias, "dirtydishes", "dirtypages"));
            let ProviderResolution::Forgejo { repo, profile } = resolution else {
                panic!("expected {alias} to resolve as Forgejo");
            };
            assert_eq!(repo.host, "git.dirtydishes.dev");
            assert_eq!(repo.owner, "dirtydishes");
            assert_eq!(repo.name, "dirtypages");
            assert_eq!(profile.canonical_host, "git.dirtydishes.dev");
        }
        Ok(())
    }

    #[test]
    fn registry_rejects_duplicate_and_ambiguous_transport_names() {
        let duplicate = HostRegistry::new(vec![HostProfile {
            canonical_host: "git.example.com".to_string(),
            aliases: vec!["git.local".to_string(), "GIT.LOCAL".to_string()],
            api_root: "https://git.example.com/api/v1".to_string(),
            credential_host: "git.example.com".to_string(),
        }]);
        let Err(error) = duplicate else {
            panic!("duplicate aliases in one profile must fail");
        };
        assert_eq!(error.message(), "duplicate transport host: git.local");

        let ambiguous = HostRegistry::new(vec![
            HostProfile {
                canonical_host: "git.one.example".to_string(),
                aliases: vec!["git.local".to_string()],
                api_root: "https://git.one.example/api/v1".to_string(),
                credential_host: "git.one.example".to_string(),
            },
            HostProfile {
                canonical_host: "git.two.example".to_string(),
                aliases: vec!["git.local".to_string()],
                api_root: "https://git.two.example/api/v1".to_string(),
                credential_host: "git.two.example".to_string(),
            },
        ]);
        let Err(error) = ambiguous else {
            panic!("one alias assigned to two profiles must fail");
        };
        assert_eq!(error.message(), "ambiguous transport host: git.local");
    }

    #[test]
    fn registry_rejects_github_and_non_http_api_roots() {
        let github = HostRegistry::new(vec![HostProfile {
            canonical_host: "github.com".to_string(),
            aliases: Vec::new(),
            api_root: "https://api.github.com".to_string(),
            credential_host: "github.com".to_string(),
        }]);
        let Err(error) = github else {
            panic!("GitHub must not be registered as Forgejo");
        };
        assert_eq!(
            error.message(),
            "GitHub host cannot be registered as Forgejo: github.com"
        );

        let non_http = HostRegistry::new(vec![HostProfile {
            canonical_host: "git.example.com".to_string(),
            aliases: Vec::new(),
            api_root: "ssh://git.example.com/api/v1".to_string(),
            credential_host: "git.example.com".to_string(),
        }]);
        let Err(error) = non_http else {
            panic!("non-HTTP API roots must fail");
        };
        assert_eq!(
            error.message(),
            "API root must use HTTP or HTTPS: ssh://git.example.com/api/v1"
        );
    }
}
