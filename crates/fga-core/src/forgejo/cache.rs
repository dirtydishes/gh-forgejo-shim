use super::User;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct SessionCache(HashMap<[u8; 32], (Instant, User)>);

impl SessionCache {
    const TTL: Duration = Duration::from_secs(60);
    const CAPACITY: usize = 256;

    pub(super) fn get(&mut self, key: &[u8; 32], now: Instant) -> Option<User> {
        self.0
            .retain(|_, (at, _)| now.duration_since(*at) < Self::TTL);
        self.0.get(key).map(|(_, user)| user.clone())
    }

    pub(super) fn insert(&mut self, key: [u8; 32], user: User, now: Instant) {
        self.0
            .retain(|_, (at, _)| now.duration_since(*at) < Self::TTL);
        if self.0.len() >= Self::CAPACITY && !self.0.contains_key(&key) {
            let oldest = *self
                .0
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .expect("nonempty cache")
                .0;
            self.0.remove(&oldest);
        }
        self.0.insert(key, (now, user));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    fn user(id: i64) -> User {
        User {
            id,
            login: format!("u{id}"),
            full_name: None,
            html_url: None,
            avatar_url: String::new(),
        }
    }
    #[test]
    fn cache_expires_after_60_seconds_without_sliding_expiration() {
        let mut cache = SessionCache::default();
        let now = Instant::now();
        cache.insert([1; 32], user(1), now);
        assert!(cache.get(&[1; 32], now + Duration::from_secs(59)).is_some());
        assert!(cache.get(&[1; 32], now + Duration::from_secs(60)).is_none());
        assert!(cache.0.is_empty());
    }
    #[test]
    fn cache_evicts_oldest_at_capacity_and_keeps_only_hashes_and_users() {
        let mut cache = SessionCache::default();
        let now = Instant::now();
        for n in 0..=SessionCache::CAPACITY {
            let key = Sha256::digest(n.to_be_bytes()).into();
            cache.insert(key, user(n as i64), now + Duration::from_millis(n as u64));
        }
        assert_eq!(cache.0.len(), SessionCache::CAPACITY);
        assert!(
            cache
                .get(
                    &Sha256::digest(0usize.to_be_bytes()).into(),
                    now + Duration::from_secs(1)
                )
                .is_none()
        );
        assert!(
            cache
                .get(
                    &Sha256::digest(1usize.to_be_bytes()).into(),
                    now + Duration::from_secs(1)
                )
                .is_some()
        );
    }
}
