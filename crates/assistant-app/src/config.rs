//! The owner's choices, kept in a small file in the app's own config
//! folder: which provider and model to use. Nothing secret is kept here;
//! the API keys live only in the encrypted key file.

use std::path::{Path, PathBuf};

use gtk::glib;

const GROUP: &str = "lookup";

/// The two providers the app can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderChoice {
    Anthropic,
    OpenRouter,
}

impl ProviderChoice {
    pub fn key(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenRouter => "openrouter",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "anthropic" => Some(Self::Anthropic),
            "openrouter" => Some(Self::OpenRouter),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// `None` until the owner has set the app up.
    pub provider: Option<ProviderChoice>,
    /// The model's id at that provider.
    pub model: String,
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join("settings.ini")
}

/// Where the encrypted key file lives, in the app's own data folder.
pub fn key_file() -> PathBuf {
    glib::user_data_dir().join("keys")
}

/// The longest model id kept, far beyond any real one.
const MAX_MODEL: usize = 200;

impl Config {
    /// Reads the choices, falling back to none for anything missing or
    /// unreadable, so a damaged file only means setting up again.
    pub fn load(path: &Path) -> Self {
        let file = glib::KeyFile::new();
        if file.load_from_file(path, glib::KeyFileFlags::NONE).is_err() {
            return Self::default();
        }
        let provider = file
            .string(GROUP, "provider")
            .ok()
            .and_then(|p| ProviderChoice::from_key(&p));
        let model = file
            .string(GROUP, "model")
            .map(|m| m.to_string())
            .unwrap_or_default();
        let model_ok = !model.is_empty()
            && model.len() <= MAX_MODEL
            && model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._/:".contains(&b));
        match provider {
            Some(provider) if model_ok => Self {
                provider: Some(provider),
                model,
            },
            _ => Self::default(),
        }
    }

    /// Saves the choices, keeping anything else the file holds.
    pub fn save(&self, path: &Path) -> Result<(), glib::Error> {
        let file = glib::KeyFile::new();
        let _ = file.load_from_file(path, glib::KeyFileFlags::KEEP_COMMENTS);
        file.set_string(GROUP, "provider", self.provider.map_or("", |p| p.key()));
        file.set_string(GROUP, "model", &self.model);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        file.save_to_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("assistant-config-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.ini")
    }

    #[test]
    fn choices_round_trip() {
        let path = scratch("round-trip");
        let wanted = Config {
            provider: Some(ProviderChoice::OpenRouter),
            model: "anthropic/claude-opus-5.5".into(),
        };
        wanted.save(&path).unwrap();
        assert_eq!(Config::load(&path), wanted);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_or_damaged_choices_mean_setting_up_again() {
        let path = scratch("damaged");
        assert_eq!(Config::load(&path), Config::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for text in [
            "[lookup]\nprovider=somewhere\nmodel=claude-opus-5-5\n",
            "[lookup]\nprovider=anthropic\nmodel=\n",
            "[lookup]\nprovider=anthropic\nmodel=claude opus; rm\n",
            "not a key file",
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(Config::load(&path), Config::default(), "{text}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
