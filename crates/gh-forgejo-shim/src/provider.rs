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
