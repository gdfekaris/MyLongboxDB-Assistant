//! The unlocked keys and the rules for holding them: the passphrase's
//! length, and forgetting the keys after a quarter of an hour without use.

use std::time::{Duration, Instant};

use assistant_keys::{ApiKey, ApiKeys};

use crate::config::ProviderChoice;
use crate::i18n::gettext_f;

/// The shortest passphrase accepted, in characters.
pub const MIN_PASSPHRASE: usize = 12;
/// How long the keys are held without the owner using the app.
pub const IDLE_LOCK: Duration = Duration::from_secs(15 * 60);

/// Why a passphrase is not accepted, or `None` if it is.
pub fn passphrase_problem(passphrase: &str) -> Option<String> {
    let length = passphrase.chars().count();
    (length < MIN_PASSPHRASE).then(|| {
        gettext_f(
            "Use at least {min} characters; this has {length}.",
            &[
                ("min", &MIN_PASSPHRASE.to_string()),
                ("length", &length.to_string()),
            ],
        )
    })
}

/// The keys while the app holds them. They are wiped from memory when
/// the session locks or ends.
#[derive(Debug)]
pub struct Session {
    keys: Option<ApiKeys>,
    /// Whether the keys are in the key file, so a passphrase can bring
    /// them back; otherwise locking forgets them for good.
    saved: bool,
    last_used: Instant,
}

impl Session {
    pub fn locked(saved: bool, now: Instant) -> Self {
        Self {
            keys: None,
            saved,
            last_used: now,
        }
    }

    pub fn unlock(&mut self, keys: ApiKeys, saved: bool, now: Instant) {
        self.keys = Some(keys);
        self.saved = saved;
        self.last_used = now;
    }

    /// Forgets the keys.
    pub fn lock(&mut self) {
        self.keys = None;
    }

    pub fn is_unlocked(&self) -> bool {
        self.keys.is_some()
    }

    /// The owner did something; the idle time starts again.
    pub fn touch(&mut self, now: Instant) {
        self.last_used = now;
    }

    /// Locks the session if it has been idle too long, returning whether
    /// it did.
    pub fn lock_if_idle(&mut self, now: Instant) -> bool {
        if self.keys.is_some() && now.saturating_duration_since(self.last_used) >= IDLE_LOCK {
            self.lock();
            return true;
        }
        false
    }

    /// The key for a provider, while unlocked.
    pub fn key(&self, provider: ProviderChoice) -> Option<&ApiKey> {
        let keys = self.keys.as_ref()?;
        match provider {
            ProviderChoice::Anthropic => keys.anthropic.as_ref(),
            ProviderChoice::OpenRouter => keys.openrouter.as_ref(),
        }
    }

    /// All the keys, for saving them again under a new passphrase.
    pub fn keys(&self) -> Option<&ApiKeys> {
        self.keys.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> ApiKeys {
        ApiKeys {
            anthropic: ApiKey::new("sk-ant-TESTONLY".into()),
            openrouter: None,
        }
    }

    #[test]
    fn passphrases_need_twelve_characters() {
        assert!(passphrase_problem("correct horse").is_none());
        assert!(passphrase_problem("éééééééééééé").is_none());
        let problem = passphrase_problem("short").unwrap();
        assert!(problem.contains("12") && problem.contains('5'));
    }

    #[test]
    fn keys_are_forgotten_after_fifteen_idle_minutes() {
        let start = Instant::now();
        let mut session = Session::locked(true, start);
        assert!(!session.lock_if_idle(start + IDLE_LOCK));
        session.unlock(keys(), true, start);
        assert!(session.key(ProviderChoice::Anthropic).is_some());
        assert!(session.key(ProviderChoice::OpenRouter).is_none());
        let almost = start + IDLE_LOCK - Duration::from_secs(1);
        assert!(!session.lock_if_idle(almost));
        session.touch(almost);
        assert!(!session.lock_if_idle(start + IDLE_LOCK));
        assert!(session.lock_if_idle(almost + IDLE_LOCK));
        assert!(!session.is_unlocked());
        assert!(session.key(ProviderChoice::Anthropic).is_none());
    }

    #[test]
    fn the_session_never_prints_its_keys() {
        let mut session = Session::locked(false, Instant::now());
        session.unlock(keys(), false, Instant::now());
        assert!(!format!("{session:?}").contains("TESTONLY"));
    }
}
