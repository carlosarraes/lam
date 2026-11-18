use std::hash::{BuildHasher, Hasher};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::client::is_transient;

pub struct Policy {
    pub deadline: Instant,
    pub max_attempts: u32,
    pub base: Duration,
    pub cap: Duration,
    pub jitter: bool,
}

/// Why `run` stopped without a value. `last` is `None` only when the deadline passed before the
/// first attempt; `transient` says whether the last error was one worth retrying.
pub struct GaveUp {
    pub attempts: u32,
    pub last: Option<anyhow::Error>,
    pub transient: bool,
}

pub fn one_line(error: &anyhow::Error) -> String {
    error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Equal jitter: half the backoff is kept, the other half is random, so retries from many
/// agents spread out without ever collapsing to zero delay.
fn jittered(backoff: Duration) -> Duration {
    let half = backoff / 2;
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(Instant::now().elapsed().as_nanos());
    let fraction = (hasher.finish() % 1_000) as u32;
    half + half * fraction / 1_000
}

/// Calls `request` until it succeeds, fails with a non-transient error, runs out of attempts,
/// or would sleep past the deadline. Prints one stderr line per retry.
pub fn run<T>(policy: &Policy, mut request: impl FnMut() -> Result<T>) -> Result<T, GaveUp> {
    let mut backoff = policy.base;
    let mut attempts = 0;
    let mut last = None;
    loop {
        if Instant::now() >= policy.deadline {
            return Err(GaveUp {
                attempts,
                last,
                transient: true,
            });
        }
        attempts += 1;
        let error = match request() {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let transient = is_transient(&error);
        let remaining = policy.deadline.saturating_duration_since(Instant::now());
        if !transient || attempts >= policy.max_attempts || remaining.is_zero() {
            return Err(GaveUp {
                attempts,
                last: Some(error),
                transient,
            });
        }
        let delay = if policy.jitter {
            jittered(backoff)
        } else {
            backoff
        }
        .min(remaining);
        let progress = if policy.max_attempts == u32::MAX {
            String::new()
        } else {
            format!(" (attempt {}/{})", attempts + 1, policy.max_attempts)
        };
        eprintln!(
            "lam: {}; retrying in {:.1}s{progress}",
            one_line(&error),
            delay.as_secs_f64()
        );
        last = Some(error);
        std::thread::sleep(delay);
        backoff = (backoff * 2).min(policy.cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_half_to_full_backoff() {
        for _ in 0..100 {
            let d = jittered(Duration::from_secs(2));
            assert!(
                d >= Duration::from_secs(1) && d <= Duration::from_secs(2),
                "{d:?}"
            );
        }
    }
}
