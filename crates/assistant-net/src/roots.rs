//! The only certificate authorities the app trusts. Both hosts are served
//! through Cloudflare, which issues their certificates from Google Trust
//! Services, Let's Encrypt, or SSL.com and may switch between them, so
//! these are those three's roots in Mozilla's store, and nothing else.
//! A certificate from any other authority is refused, even one a browser
//! would accept.
//!
//! `roots/SHA256SUMS` lists each file's SHA-256 fingerprint, to compare
//! with Mozilla's published list: `sha256sum -c SHA256SUMS` in that
//! folder.

/// Each root's name and DER bytes.
pub const ROOTS: [(&str, &[u8]); 9] = [
    ("GTS Root R1", include_bytes!("roots/gts-root-r1.der")),
    ("GTS Root R3", include_bytes!("roots/gts-root-r3.der")),
    ("GTS Root R4", include_bytes!("roots/gts-root-r4.der")),
    ("ISRG Root X1", include_bytes!("roots/isrg-root-x1.der")),
    ("ISRG Root X2", include_bytes!("roots/isrg-root-x2.der")),
    (
        "SSL.com Root Certification Authority RSA",
        include_bytes!("roots/sslcom-root-rsa.der"),
    ),
    (
        "SSL.com Root Certification Authority ECC",
        include_bytes!("roots/sslcom-root-ecc.der"),
    ),
    (
        "SSL.com TLS RSA Root CA 2022",
        include_bytes!("roots/sslcom-tls-rsa-2022.der"),
    ),
    (
        "SSL.com TLS ECC Root CA 2022",
        include_bytes!("roots/sslcom-tls-ecc-2022.der"),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// ureq skips a root it cannot parse without saying so, which would
    /// quietly narrow what the app trusts to nothing at all.
    #[test]
    fn every_root_is_a_usable_trust_anchor() {
        let mut store = rustls::RootCertStore::empty();
        let (added, ignored) = store.add_parsable_certificates(
            ROOTS
                .iter()
                .map(|(_, der)| rustls::pki_types::CertificateDer::from(*der)),
        );
        assert_eq!((added, ignored), (ROOTS.len(), 0));
    }

    #[test]
    fn each_root_is_named_in_its_own_subject() {
        for (name, der) in ROOTS {
            assert!(
                der.windows(name.len()).any(|w| w == name.as_bytes()),
                "{name}"
            );
        }
    }
}
