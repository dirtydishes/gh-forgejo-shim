//! One monotonic time budget shared by all work in a command.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CommandDeadline {
    expires_at: Instant,
}

impl CommandDeadline {
    pub fn after(duration: Duration) -> Self {
        let started = Instant::now();
        Self {
            expires_at: started.checked_add(duration).unwrap_or(started),
        }
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.expires_at
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
    }
}
