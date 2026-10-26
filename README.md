# MyLongboxDB Assistant

A Linux desktop app that fills in the details of comic books for
[MyLongboxDB](https://github.com/gdfekaris/MyLongboxDB). Give it a cover
photo, or a series and issue number, and a Claude model, called with
your own Anthropic API key, looks the book up. You review every field it
found, then the app writes the books as a MyLongboxDB full export, which
MyLongboxDB imports like any other. Magazines and collected editions
have their fields, but the lookup has not yet been tuned for them.

The Assistant never opens your MyLongboxDB collection. MyLongboxDB
itself stays offline; this separate app holds the network permission
instead, and only ever sees the photos and text you give it.

**Status:** in development. There is no release yet, and the app does
not do anything useful so far.

## Building

MyLongboxDB Assistant is written in Rust with GTK4 and libadwaita and
ships only as a Flatpak.

### Flatpak

```sh
flatpak install --user flathub org.gnome.Sdk//51 \
    org.freedesktop.Sdk.Extension.rust-stable//26.08 org.flatpak.Builder
flatpak run org.flatpak.Builder --user --install --force-clean \
    _build build-aux/io.github.gdfekaris.MyLongboxDB.Assistant.yml
flatpak run io.github.gdfekaris.MyLongboxDB.Assistant
```

The build is fully offline: every crate is listed with its checksum in
`build-aux/cargo-sources.json`, which is generated from `Cargo.lock` by
[flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo)
and must be regenerated whenever the lockfile changes:

```sh
flatpak run --filesystem="$PWD" --command=flatpak-cargo-generator \
    org.flatpak.Builder "$PWD/Cargo.lock" -o "$PWD/build-aux/cargo-sources.json"
```

### Development

The Rust version is pinned in `rust-toolchain.toml`. The app links
against GTK4 and libadwaita from the GNOME 51 SDK rather than the host
system, so Cargo runs inside the SDK with the pinned toolchain:

```sh
# Download crates on the host; the SDK shell below has no network.
cargo fetch

sdk() {
    flatpak run --socket=wayland --socket=fallback-x11 --device=dri \
        --filesystem="$PWD" --filesystem=~/.cargo --filesystem=~/.rustup \
        --env=PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
        --env=CARGO_HOME="$HOME/.cargo" --env=RUSTUP_HOME="$HOME/.rustup" \
        --env=CARGO_TARGET_DIR="$PWD/target/sdk" --env=CARGO_NET_OFFLINE=true \
        --cwd="$PWD" --command=sh org.gnome.Sdk//51 -c "$*"
}

sdk cargo clippy --workspace --all-targets --all-features -- -D warnings
sdk cargo test --workspace
```

The supply-chain checks run on the host:

```sh
cargo fmt --all --check
cargo deny fetch && cargo deny check
cargo vet --locked
```

### Translations

Text shown in the app is marked in the Rust code with the functions in
`crates/assistant-app/src/i18n.rs`, and the review page's field names
in `crates/assistant-core/src/lookup/fields.rs`. The template for
translators, `po/mylongboxdb-assistant.pot`, is made from the files
listed in `po/POTFILES`:

```sh
sdk "grep '\.rs\$' po/POTFILES | xgettext --files-from=- --language=Rust \
    --from-code=UTF-8 --add-comments=Translators --keyword=gettext \
    --keyword=gettext_f --keyword=pgettext:1c,2 --keyword=field:2 --keyword=edition:2 \
    --keyword=judgment:2 \
    --package-name=mylongboxdb-assistant \
    --msgid-bugs-address=https://github.com/gdfekaris/MyLongboxDB-Assistant/issues \
    --sort-by-file -o po/mylongboxdb-assistant.pot"
```

A translation is a `po/<language>.po` file made from the template, with
its language code added to `po/LINGUAS`; the Flatpak build checks and
compiles each one.

## What is sent where

The app connects to one host, over HTTPS: `api.anthropic.com`. Its network
layer also allows `openrouter.ai`, for OpenRouter support planned for a
later version, but this version never contacts it.

- **Sent to Anthropic:** each cover photo you add, re-encoded from its
  pixels so it carries no EXIF, GPS, or other metadata; the series and
  issue lines you type; and the app's fixed instructions.
- **Looked up at Anthropic:** to research a book, the model searches
  the web and may read whole issue pages from the Marvel and DC wikis
  at marvel.fandom.com and dc.fandom.com and from marvel.com, dc.com,
  and imagecomics.com (the `READ_SITES` constant in
  `crates/assistant-net/src/lookup.rs`). Anthropic's servers do the
  searching and reading; the app itself never fetches a web page.
- **Never sent:** your MyLongboxDB collection, file names, or anything
  about your computer beyond what any HTTPS request carries.

Use a dedicated API key for this app, in an Anthropic workspace with a
monthly spend limit, and revoke it at the first doubt.

## Security model

Each claim below comes with a way to check it without trusting the
authors.

- **Only crates from crates.io, and only one way onto the network.**
  `deny.toml` bans every HTTP, socket, and TLS crate except ureq,
  rustls, and ring, and allows those only inside the app's HTTP crate.
  No other root-certificate store or crypto provider may be pulled in.
  Verify with `cargo deny check`.
- **Two hosts, nine certificate authorities.** The network crate,
  `crates/assistant-net`, can address only `api.anthropic.com` and
  `openrouter.ai` (the `HOSTS` constant in `src/http.rs`), over HTTPS on
  port 443, and checks every request again just before it is sent. It
  follows no redirects, ignores proxy settings in the environment, and
  caps the size and time of each exchange. TLS is rustls with ring,
  limited to TLS 1.3, and trusts only the root certificates in
  `src/roots/`: those of Google Trust Services, Let's Encrypt, and
  SSL.com, the three authorities Cloudflare, which serves both hosts,
  issues from. A certificate from any other authority is refused, even
  one a browser would accept. Verify the certificates with
  `sha256sum -c SHA256SUMS` in `crates/assistant-net/src/roots/`,
  comparing the fingerprints with Mozilla's root store, and the rest with
  `cargo test -p assistant-net`.
- **API keys are kept encrypted, or not at all.** A saved key is in
  one file, `~/.var/app/io.github.gdfekaris.MyLongboxDB.Assistant/data/keys`,
  readable only by you (mode 0600). XChaCha20-Poly1305 encrypts it under
  a key that Argon2id derives from your passphrase (512 MiB, 3 passes,
  about a second to unlock), and the file's header, which holds those
  settings, is authenticated, so the file cannot be weakened unnoticed;
  a file whose settings are below 64 MiB and 3 passes is refused. A
  passphrase has at least 12 characters. The app forgets the key after
  15 minutes unused. Keys never appear in error messages or printed
  output. Verify with `cargo test -p assistant-keys`, whose tests change
  the file's bytes, weaken its settings, and look for key text in the
  file and in what the app prints, and with `stat -c %a` on the file.
  This protects a copied or backed-up key file and keeps the key from
  other sandboxed apps; it cannot protect against malware already
  running as you, which could watch the passphrase being typed. The
  real limit there is at the provider: give the app its own key with a
  monthly spending limit, and revoke it at the first doubt.
- **Every dependency is reviewed or deliberately accepted.** cargo-vet
  records, in `build-aux/supply-chain/`, which crates were audited by
  organisations whose audits are imported (Mozilla, Google, the Bytecode
  Alliance, and others) and which were accepted at a given version; a
  new crate or a new version fails the check until someone looks at it.
  Verify with `cargo vet --locked`.
- **No running other programs, no raw sockets.** `std::process`, GLib's
  spawn functions, GIO subprocesses, GTK's URI and file launchers, and
  `std::net`'s sockets are banned through `clippy.toml`. Verify with
  `sdk cargo clippy --workspace --all-targets -- -D warnings`.
- **`unsafe` code in one small crate only.** Every crate declares
  `#![forbid(unsafe_code)]` except `crates/assistant-sandbox`, which
  holds the system-level code below and the gettext setup; each of its
  `unsafe` blocks says why it is sound. Verify with
  `grep -rL 'forbid(unsafe_code)' crates/*/src/lib.rs crates/*/src/main.rs`,
  which should list only that crate.
- **The app limits itself as it starts.** Before any other thread
  starts, it applies Landlock rules so that it can change files only in
  its own folders and the folder through which the file chooser hands
  over saved files, can connect over TCP only to port 443, and can
  never listen for connections; and a seccomp filter that refuses
  sockets other than local, IPv4, and IPv6 ones, and io_uring.
  These hold even if other code in the app misbehaves, though not
  against code that has taken over the whole process. Where the kernel
  cannot enforce one of them, the app says which at the top of its
  window, and carries on inside the Flatpak sandbox. Verify with
  `cargo test -p assistant-sandbox`, whose tests apply each limit and
  try to break it. Kernels before Landlock ABI 4 cannot limit TCP; with
  `ASSISTANT_REQUIRE_LANDLOCK_NET=1` set, the test fails on such a
  kernel instead of skipping that part. While the app runs,
  `grep Seccomp_filters /proc/$(pgrep -n -f '^mylongboxdb-assistant')/status`
  shows 2: Flatpak's own filter and the app's.
- **Photos carry nothing but their pixels.** Every photo you pick is
  checked against MyLongboxDB's limits (50 MB, 100 megapixels, 40,000
  pixels a side) before it is decoded, then re-encoded from its pixels
  alone, so no EXIF, GPS, XMP, IPTC, comment, or colour-profile data
  reaches the provider or the export. Verify with
  `cargo test -p assistant-media`, whose tests hide marked text in each
  kind of metadata and check that none of it survives, and by running
  `exiftool` or ImageMagick's `identify -verbose` on a photo in an
  export, which shows no camera or location.
- **Photos are decoded on a locked-down thread.** The JPEG, PNG, WebP,
  and HEIC decoders run on a thread that can open no file, make or use
  no socket, write only to standard output and error, and start no
  program, so a flaw in a decoder has little to reach. Photos the app
  saved itself are read back only as JPEGs of its own naming and size.
  Verify with `cargo test -p assistant-sandbox --test confine` and
  `cargo test -p assistant-media`, which decodes each format on such a
  thread. HEIC needs libheif with an HEVC decoder, which the Flatpak
  build adds, so its tests run inside the GNOME SDK after a Flatpak
  build, with `LD_LIBRARY_PATH=$PWD/_build/files/lib` set and
  `cargo test -p assistant-media --features heic -- --include-ignored`.
- **What the model says is not trusted.** Each answer must match a
  strict schema, which refuses unknown fields, wrong types, and
  oversized lists. A value is kept only if it cites a page the
  provider's web search or page reading actually returned, what was read
  from your own cover, or the model's own knowledge, and a separate
  check step compares each value with what its source says. A value from
  the model's knowledge, with no page behind it, says so on the review
  page, and is ticked only when the model is sure of it. Every value
  then passes the checks MyLongboxDB's import applies, and nothing a
  model says can choose a file, a host, or a setting. You review
  everything before export, and values you leave unticked are left out.
  Verify with `cargo test -p assistant-core`, and with the CI job that
  imports the app's exports using MyLongboxDB's own code at a pinned
  release.
- **No telemetry, crash reports, or update checks.** The app can reach
  only the two provider hosts above, so there is nowhere else for them
  to go. Verify with `cargo test -p assistant-net`, and with
  `grep -n HOSTS crates/assistant-net/src/http.rs`.
- **Your results and exports never hold a key.** Looked-up books wait
  in `~/.var/app/io.github.gdfekaris.MyLongboxDB.Assistant/data/results.json`,
  with your photos beside it, until you export or discard them. That
  file and the export are written by `crates/assistant-core`, which
  does not depend on the key or network code at all, so neither can
  contain a key. Verify with
  `cargo tree -p assistant-core -e normal | grep -E 'assistant-(keys|net)'`,
  which prints nothing.
- **Network and display only.** The Flatpak manifest grants exactly
  `--share=network`, `--socket=wayland`, `--socket=fallback-x11`,
  `--share=ipc`, and `--device=dri`: no access to your home folder or
  the host filesystem. Verify with
  `flatpak info --show-permissions io.github.gdfekaris.MyLongboxDB.Assistant`.

## Reporting vulnerabilities

Please report security issues privately through GitHub's
"Report a vulnerability" form on the repository's Security tab rather
than in a public issue.

## License

MIT. See [LICENSE](LICENSE).
