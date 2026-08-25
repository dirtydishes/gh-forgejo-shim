//! Canonical Forgejo host profiles and transport-alias resolution.

use crate::config::{is_known_github_host, normalize_host};
use crate::repo::RepoRef;
use crate::Result;

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
    Forgejo {
        repo: RepoRef,
        profile: HostProfile,
    },
    GitHub {
        repo: RepoRef,
    },
    Unconfigured {
        repo: RepoRef,
    },
}

impl HostRegistry {
    pub fn new(profiles: Vec<HostProfile>) -> Result<Self> {
        Ok(Self { profiles })
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
}
