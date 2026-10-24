# MyLongboxDB Assistant

A Linux desktop app that fills in the details of comic books, magazines,
and collected editions for
[MyLongboxDB](https://github.com/gdfekaris/MyLongboxDB). Give it a cover
photo, or a series and issue number, and an AI model, called with your
own API key from Anthropic or OpenRouter, looks the book up. You review
every field it found, then the app writes the books as a MyLongboxDB
full export, which MyLongboxDB imports like any other.

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

## What is sent where

The app connects to exactly two hosts, both over HTTPS: `api.anthropic.com`
if you choose Anthropic, and `openrouter.ai` if you choose OpenRouter.

- **Sent to the provider you choose:** each cover photo you add,
  re-encoded from its pixels so it carries no EXIF, GPS, or other
  metadata; the series and issue lines you type; and the app's fixed
  instructions. OpenRouter passes these on to the company that runs the
  model you pick.
- **Never sent:** your MyLongboxDB collection, file names, or anything
  about your computer beyond what any HTTPS request carries.

Use a dedicated API key for this app with a monthly spending limit (an
Anthropic workspace spend limit, or an OpenRouter per-key credit limit),
and revoke it at the first doubt.

## Security model

Each claim below comes with a way to check it without trusting the
authors. The list grows as the app is built.

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
  sockets other than local, IPv4, IPv6, and netlink ones, and io_uring.
  These hold even if other code in the app misbehaves, though not
  against code that has taken over the whole process. Verify with
  `cargo test -p assistant-sandbox`, whose tests apply each limit and
  try to break it. Kernels before Landlock ABI 4 cannot limit TCP; with
  `ASSISTANT_REQUIRE_LANDLOCK_NET=1` set, the test fails on such a
  kernel instead of skipping that part.
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
