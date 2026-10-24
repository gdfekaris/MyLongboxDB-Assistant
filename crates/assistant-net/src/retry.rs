//! Trying a request again when the provider is busy: on rate limits,
//! overload, and server errors, waiting as long as the provider asks or
//! backing off, and never after the request may have been carried out,
//! which would be paid for twice.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::http::{Client, NetError, Request, Response};

/// Tries in all, including the first.
pub const MAX_ATTEMPTS: u32 = 4;
/// The longest wait between tries, whatever the provider asks.
const MAX_WAIT: Duration = Duration::from_secs(120);

/// How long to wait before trying again after `attempt` (counting from 1)
/// ended this way, or `None` to give up.
pub fn delay(outcome: &Result<Response, NetError>, attempt: u32) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let backoff = Duration::from_secs(2u64.saturating_pow(attempt)).min(MAX_WAIT);
    match outcome {
        Ok(response) => match response.status {
            408 | 429 | 500 | 502 | 503 | 504 | 529 => {
                Some(response.retry_after.map_or(backoff, |r| r.min(MAX_WAIT)))
            }
            _ => None,
        },
        // Nothing reached the provider, so trying again costs nothing.
        Err(NetError::Unreachable) => Some(backoff),
        // A timeout or a dropped connection may come after the provider
        // did the work, so it is not repeated.
        Err(_) => None,
    }
}

/// Waits `duration`, checking `cancel` often so the owner is not kept
/// waiting. Returns whether it was cancelled.
fn wait(duration: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + duration;
    while Instant::now() < end {
        if cancel.load(Ordering::Relaxed) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
    }
    cancel.load(Ordering::Relaxed)
}

/// Sends the request `make` builds, again if the provider is busy, until
/// it answers, the tries run out, or `cancel` is set.
pub fn send(
    client: &Client,
    make: impl Fn() -> Request,
    cancel: &AtomicBool,
) -> Result<Response, NetError> {
    let mut attempt = 1;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(NetError::Cancelled);
        }
        let outcome = client.send(make());
        match delay(&outcome, attempt) {
            Some(wait_for) => {
                if wait(wait_for, cancel) {
                    return Err(NetError::Cancelled);
                }
                attempt += 1;
            }
            None => return outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: u16, retry_after: Option<u64>) -> Result<Response, NetError> {
        Ok(Response {
            status,
            retry_after: retry_after.map(Duration::from_secs),
            body: Vec::new(),
        })
    }

    #[test]
    fn busy_providers_are_tried_again_with_backoff() {
        assert_eq!(delay(&status(429, None), 1), Some(Duration::from_secs(2)));
        assert_eq!(delay(&status(529, None), 2), Some(Duration::from_secs(4)));
        assert_eq!(delay(&status(503, None), 3), Some(Duration::from_secs(8)));
        assert_eq!(delay(&status(503, None), MAX_ATTEMPTS), None);
    }

    #[test]
    fn the_provider_s_wait_is_honoured_up_to_a_limit() {
        assert_eq!(
            delay(&status(429, Some(30)), 1),
            Some(Duration::from_secs(30))
        );
        assert_eq!(delay(&status(429, Some(3600)), 1), Some(MAX_WAIT));
    }

    #[test]
    fn answers_and_mistakes_are_not_repeated() {
        for code in [200, 400, 401, 402, 403, 404, 413] {
            assert_eq!(delay(&status(code, Some(1)), 1), None, "{code}");
        }
    }

    #[test]
    fn only_failures_before_the_provider_are_repeated() {
        assert_eq!(
            delay(&Err(NetError::Unreachable), 1),
            Some(Duration::from_secs(2))
        );
        for error in [
            NetError::TimedOut,
            NetError::Network,
            NetError::Tls,
            NetError::Refused,
            NetError::TooLarge,
        ] {
            assert_eq!(delay(&Err(error.clone()), 1), None, "{error:?}");
        }
    }

    #[test]
    fn waiting_stops_when_cancelled() {
        let cancel = AtomicBool::new(true);
        let started = Instant::now();
        assert!(wait(Duration::from_secs(60), &cancel));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
