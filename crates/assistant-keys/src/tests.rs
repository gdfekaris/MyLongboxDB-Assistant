use super::*;

/// Made-up keys in the providers' shapes; none was ever issued.
fn keys() -> ApiKeys {
    ApiKeys {
        anthropic: ApiKey::new("sk-ant-api03-TESTONLY-notarealkey-0000".into()),
        openrouter: ApiKey::new("  sk-or-v1-TESTONLY-notarealkey-1111\n".into()),
    }
}

fn passphrase(text: &str) -> Passphrase {
    Passphrase::new(text.into()).unwrap()
}

/// Sealed with the weakest settings allowed, so tests run quickly.
fn sealed() -> Vec<u8> {
    seal(&keys(), &passphrase("correct horse"), KdfParams::MINIMUM).unwrap()
}

fn cannot_open(result: Result<ApiKeys, KeyFileError>) -> bool {
    matches!(result, Err(KeyFileError::CannotOpen))
}

#[test]
fn keys_come_back_with_the_right_passphrase() {
    let file = sealed();
    let opened = open(&file, &passphrase("correct horse")).unwrap();
    assert_eq!(opened, keys());
    assert_eq!(
        opened.openrouter.unwrap().expose(),
        "sk-or-v1-TESTONLY-notarealkey-1111"
    );
}

#[test]
fn the_default_settings_open_too() {
    let pass = passphrase("correct horse");
    let file = seal(&keys(), &pass, KdfParams::DEFAULT).unwrap();
    assert_eq!(open(&file, &pass).unwrap(), keys());
}

#[test]
fn one_key_or_none_is_fine() {
    let pass = passphrase("p");
    for held in [
        ApiKeys {
            anthropic: None,
            ..keys()
        },
        ApiKeys::default(),
    ] {
        let file = seal(&held, &pass, KdfParams::MINIMUM).unwrap();
        assert_eq!(open(&file, &pass).unwrap(), held);
    }
}

#[test]
fn a_wrong_passphrase_is_refused() {
    let file = sealed();
    assert!(cannot_open(open(&file, &passphrase("correct horse "))));
    assert!(cannot_open(open(&file, &passphrase("Correct horse"))));
}

#[test]
fn changed_bytes_are_refused_with_the_same_error() {
    let file = sealed();
    let pass = passphrase("correct horse");
    let salt = HEADER_LEN - SALT_LEN;
    let nonce = HEADER_LEN;
    let sealed_at = HEADER_LEN + NONCE_LEN;
    // Every byte of the header before the salt fails before any key is
    // derived; past it, each changed byte costs a derivation, so the
    // salt, nonce, ciphertext, and tag are sampled at their ends and
    // middle.
    let header = (0..salt).filter(|at| *at != MAGIC.len());
    let sampled = [
        salt,
        salt + SALT_LEN / 2,
        nonce - 1,
        nonce,
        nonce + NONCE_LEN / 2,
        sealed_at - 1,
        sealed_at,
        sealed_at + (file.len() - sealed_at) / 2,
        file.len() - 16,
        file.len() - 1,
    ];
    for at in header.chain(sampled) {
        let mut changed = file.clone();
        changed[at] ^= 0x01;
        assert!(cannot_open(open(&changed, &pass)), "byte {at}");
    }
}

#[test]
fn a_later_format_is_named_and_an_earlier_one_refused() {
    let pass = passphrase("correct horse");
    let mut later = sealed();
    later[MAGIC.len()] = VERSION + 1;
    assert!(matches!(
        open(&later, &pass),
        Err(KeyFileError::NewerVersion)
    ));
    let mut earlier = sealed();
    earlier[MAGIC.len()] = 0;
    assert!(cannot_open(open(&earlier, &pass)));
}

#[test]
fn cut_or_lengthened_files_are_refused() {
    let file = sealed();
    let pass = passphrase("correct horse");
    for len in [0, 8, HEADER_LEN, HEADER_LEN + NONCE_LEN, file.len() - 1] {
        assert!(cannot_open(open(&file[..len], &pass)), "{len} bytes");
    }
    let mut longer = file.clone();
    longer.push(0);
    assert!(cannot_open(open(&longer, &pass)));
}

/// A file whose header states the given settings, with the rest left as
/// sealed.
fn with_settings(memory_kib: u32, passes: u32, lanes: u32) -> Vec<u8> {
    let mut file = sealed();
    let at = MAGIC.len() + 1;
    for (n, value) in [memory_kib, passes, lanes].into_iter().enumerate() {
        file[at + 4 * n..at + 4 * n + 4].copy_from_slice(&value.to_le_bytes());
    }
    file
}

#[test]
fn weakened_or_excessive_settings_are_refused_before_any_work() {
    let pass = passphrase("correct horse");
    for (m, t, p) in [
        (8, 1, 1),
        (64 * 1024 - 1, 3, 1),
        (64 * 1024, 2, 1),
        (64 * 1024, 3, 0),
        (u32::MAX, 3, 1),
        (64 * 1024, u32::MAX, 1),
        (64 * 1024, 3, 9),
    ] {
        let file = with_settings(m, t, p);
        let started = std::time::Instant::now();
        assert!(cannot_open(open(&file, &pass)));
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        assert_eq!(KdfParams::new(m, t, p), None);
    }
}

#[test]
fn each_file_has_its_own_salt_and_nonce() {
    let (a, b) = (sealed(), sealed());
    let salt = HEADER_LEN - SALT_LEN..HEADER_LEN;
    let nonce = HEADER_LEN..HEADER_LEN + NONCE_LEN;
    assert_ne!(a[salt.clone()], b[salt]);
    assert_ne!(a[nonce.clone()], b[nonce]);
}

#[test]
fn the_keys_are_not_in_the_file() {
    let file = sealed();
    for needle in [&b"sk-ant"[..], b"sk-or", b"TESTONLY", b"correct horse"] {
        assert!(!file.windows(needle.len()).any(|w| w == needle));
    }
}

#[test]
fn secrets_never_print() {
    let shown = format!(
        "{:?} {:?} {:?}",
        keys(),
        passphrase("correct horse"),
        open(&sealed(), &passphrase("wrong"))
    );
    assert!(!shown.contains("sk-"));
    assert!(!shown.contains("TESTONLY"));
    assert!(!shown.contains("correct horse"));
    assert!(!shown.contains("wrong"));
    assert!(shown.contains("hidden"));
    let error = open(&sealed(), &passphrase("wrong")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the passphrase is wrong or the key file is damaged"
    );
}

#[test]
fn keys_and_passphrases_are_checked() {
    assert!(ApiKey::new(String::new()).is_none());
    assert!(ApiKey::new("   ".into()).is_none());
    assert!(ApiKey::new("sk-ant has spaces".into()).is_none());
    assert!(ApiKey::new("sk-ant-é".into()).is_none());
    assert!(ApiKey::new("k".repeat(MAX_KEY_BYTES + 1)).is_none());
    assert!(ApiKey::new("k".repeat(MAX_KEY_BYTES)).is_some());
    assert!(Passphrase::new(String::new()).is_none());
    assert!(Passphrase::new(" ".into()).is_some());
}

/// A new, empty folder for one test.
fn folder() -> std::path::PathBuf {
    let suffix: [u8; 8] = random().unwrap();
    let name: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join(format!("assistant-keys-{name}"));
    std::fs::create_dir(&dir).unwrap();
    dir
}

#[test]
fn saved_files_are_private_and_replace_the_old_one_whole() {
    use std::os::unix::fs::PermissionsExt;

    let dir = folder();
    let path = dir.join("keys");
    assert!(load(&path).unwrap().is_none());
    save(&path, b"first").unwrap();
    save(&path, &sealed()).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let loaded = load(&path).unwrap().unwrap();
    assert_eq!(open(&loaded, &passphrase("correct horse")).unwrap(), keys());
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(left.len(), 1, "no temporary file is left behind");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_oversized_file_is_not_read() {
    let dir = folder();
    let path = dir.join("keys");
    std::fs::write(&path, vec![0; 64 * 1024 + 1]).unwrap();
    assert!(matches!(load(&path), Err(KeyFileError::CannotOpen)));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// How long opening takes, with the default settings or those in
/// `ASSISTANT_KDF` as `memory_kib,passes,lanes`. Run on the owner's
/// machine, in release, to tune the defaults to about a second:
/// `cargo test --release -p assistant-keys -- --ignored --nocapture`.
#[test]
#[ignore]
fn time_to_open() {
    let params = std::env::var("ASSISTANT_KDF")
        .ok()
        .map_or(KdfParams::DEFAULT, |text| {
            let n: Vec<u32> = text.split(',').map(|n| n.trim().parse().unwrap()).collect();
            KdfParams::new(n[0], n[1], n[2]).expect("settings out of range")
        });
    let pass = passphrase("correct horse");
    let file = seal(&keys(), &pass, params).unwrap();
    let runs = 5;
    let started = std::time::Instant::now();
    for _ in 0..runs {
        open(&file, &pass).unwrap();
    }
    let each = started.elapsed() / runs;
    println!(
        "{} KiB, {} passes, {} lanes: {each:?} to open",
        params.memory_kib, params.passes, params.lanes
    );
}

mod properties {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Any bytes are refused without a crash; random bytes almost never
        /// get past the header, so this is quick.
        #[test]
        fn any_file_is_refused_safely(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
            let _ = open(&bytes, &passphrase("correct horse"));
        }

        /// What was decrypted is unpacked or refused, never a crash, and
        /// only as valid keys.
        #[test]
        fn any_plaintext_is_unpacked_safely(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
            if let Ok(keys) = keys_from(&bytes) {
                for key in [keys.anthropic, keys.openrouter].into_iter().flatten() {
                    prop_assert!(ApiKey::new(key.expose().to_owned()).as_ref() == Some(&key));
                }
            }
        }
    }

    proptest! {
        // Each case derives a key at the minimum settings, 64 MiB.
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// A sealed file with any bytes changed never opens.
        #[test]
        fn a_changed_file_never_opens(
            edits in prop::collection::vec((any::<prop::sample::Index>(), 1..=255u8), 1..4),
        ) {
            let mut file = sealed();
            for (at, flip) in edits {
                let i = at.index(file.len());
                file[i] ^= flip;
            }
            prop_assert!(open(&file, &passphrase("correct horse")).is_err());
        }
    }
}
