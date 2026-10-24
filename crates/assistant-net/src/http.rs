//! The app's only way onto the network: HTTPS on port 443 to the two
//! provider hosts, over rustls with TLS 1.3, trusting only the root
//! certificates the app carries. No proxy from the environment, no
//! redirects, and capped sizes and times.
//!
//! Errors say what kind of failure happened and never carry a request's
//! headers or body, where an API key could be.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ureq::http::{self, Uri};
use ureq::middleware::MiddlewareNext;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};
use ureq::{Agent, Body, SendBody};
use zeroize::Zeroizing;

use crate::roots::ROOTS;

/// Every host the app may reach. Nothing else is ever contacted.
pub const HOSTS: [&str; 2] = ["api.anthropic.com", "openrouter.ai"];

/// A host the app may reach, the only way a request names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Anthropic,
    OpenRouter,
}

impl Host {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Anthropic => HOSTS[0],
            Self::OpenRouter => HOSTS[1],
        }
    }
}

/// The largest request body sent; Anthropic accepts up to 32 MB.
pub const MAX_REQUEST_BYTES: usize = 24 * 1024 * 1024;
/// The largest response body read.
pub const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
/// The largest response header block read.
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// How long one request may take in all. A lookup with web search runs
/// several searches before it answers.
const TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a request failed. Shown to the owner; it never holds headers or
/// bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    /// The request would have gone somewhere other than the two hosts.
    Refused,
    /// The host could not be found or reached.
    Unreachable,
    /// The secure connection failed, such as when a certificate does not
    /// lead to a trusted root.
    Tls,
    TimedOut,
    /// The request or response was larger than allowed.
    TooLarge,
    /// The provider answered with an error. `kind` and `message` come
    /// from its error body, with any API key taken out.
    Status {
        status: u16,
        kind: String,
        message: String,
        retry_after: Option<Duration>,
    },
    /// The provider's answer could not be read as what was asked for.
    BadResponse(String),
    /// The request itself was not one the app may make; a mistake in
    /// the app, not the provider.
    Invalid(&'static str),
    /// The owner cancelled.
    Cancelled,
    /// Any other failure of the connection.
    Network,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused => f.write_str("the request was refused: it was not to a provider's API"),
            Self::Unreachable => f.write_str("the provider could not be reached"),
            Self::Tls => f.write_str("the secure connection to the provider failed"),
            Self::TimedOut => f.write_str("the provider took too long to answer"),
            Self::TooLarge => f.write_str("the request or its answer was too large"),
            Self::Status {
                status,
                kind,
                message,
                ..
            } => {
                write!(f, "the provider answered with error {status}")?;
                if !kind.is_empty() {
                    write!(f, " ({kind})")?;
                }
                if !message.is_empty() {
                    write!(f, ": {message}")?;
                }
                Ok(())
            }
            Self::BadResponse(why) => write!(f, "the provider's answer could not be read: {why}"),
            Self::Invalid(why) => write!(f, "the request could not be made: {why}"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Network => f.write_str("the connection to the provider failed"),
        }
    }
}

impl std::error::Error for NetError {}

fn net_error(e: ureq::Error) -> NetError {
    match e {
        ureq::Error::Timeout(_) => NetError::TimedOut,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => NetError::Unreachable,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::TlsRequired => NetError::Tls,
        ureq::Error::BodyExceedsLimit(_) | ureq::Error::LargeResponseHeader(..) => {
            NetError::TooLarge
        }
        ureq::Error::BadUri(_) | ureq::Error::RequireHttpsOnly(_) => NetError::Refused,
        _ => NetError::Network,
    }
}

/// Whether a request is to one of [`HOSTS`] over HTTPS on port 443.
pub(crate) fn allowed(uri: &Uri) -> bool {
    uri.scheme_str() == Some("https")
        && uri.host().is_some_and(|h| HOSTS.contains(&h))
        && uri.port_u16().is_none_or(|p| p == 443)
        && uri.authority().is_some_and(|a| !a.as_str().contains('@'))
}

/// Checks every request the agent is about to send, whatever built it,
/// as a second line behind [`Host`].
struct HostGuard;

impl ureq::middleware::Middleware for HostGuard {
    fn handle(
        &self,
        request: http::Request<SendBody>,
        next: MiddlewareNext,
    ) -> Result<http::Response<Body>, ureq::Error> {
        if !allowed(request.uri()) {
            return Err(ureq::Error::BadUri("refused".into()));
        }
        next.handle(request)
    }
}

/// rustls with ring, offering only TLS 1.3: without a TLS 1.2 cipher
/// suite, no older protocol can be agreed.
pub(crate) fn crypto_provider() -> rustls::crypto::CryptoProvider {
    let mut provider = rustls::crypto::ring::default_provider();
    provider
        .cipher_suites
        .retain(|s| s.version() == &rustls::version::TLS13);
    provider
}

fn root_certs() -> RootCerts {
    let certs: Vec<Certificate<'static>> = ROOTS
        .iter()
        .map(|(_, der)| Certificate::from_der(der))
        .collect();
    RootCerts::Specific(Arc::new(certs))
}

/// An HTTP method the providers' APIs need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// A request to one of the hosts. Header values are wiped from memory
/// when the request is dropped, since one of them holds the API key.
pub struct Request {
    pub host: Host,
    pub method: Method,
    /// The path and query, starting with `/`.
    pub path: String,
    pub headers: Vec<(&'static str, Zeroizing<String>)>,
    pub body: Option<Vec<u8>>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("host", &self.host)
            .field("method", &self.method)
            .field("path", &self.path)
            .field(
                "headers",
                &self.headers.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            )
            .field("body", &self.body.as_ref().map(Vec::len))
            .finish()
    }
}

/// What came back: the status, how long the provider asked to wait
/// before trying again, and the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

/// A `Retry-After` value given in whole seconds, as both providers send
/// it.
pub(crate) fn retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Whether a path can follow the host in a URL without changing where it
/// goes.
fn safe_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'#' | b'@' | b'\\'))
}

/// The HTTPS client, made once and shared by the providers' clients.
#[derive(Clone)]
pub struct Client {
    agent: Agent,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Client")
    }
}

impl Client {
    pub fn new() -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(root_certs())
            .unversioned_rustls_crypto_provider(Arc::new(crypto_provider()))
            .build();
        let config = Agent::config_builder()
            .https_only(true)
            .proxy(None)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent("MyLongboxDB-Assistant")
            .max_response_header_size(MAX_HEADER_BYTES)
            .timeout_global(Some(TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .tls_config(tls)
            .middleware(HostGuard)
            .build();
        Self {
            agent: Agent::new_with_config(config),
        }
    }

    /// Sends `request` once. A redirect is treated as an error rather
    /// than followed.
    pub fn send(&self, request: Request) -> Result<Response, NetError> {
        if !safe_path(&request.path) {
            return Err(NetError::Refused);
        }
        if request
            .body
            .as_ref()
            .is_some_and(|b| b.len() > MAX_REQUEST_BYTES)
        {
            return Err(NetError::TooLarge);
        }
        let uri: Uri = format!("https://{}{}", request.host.name(), request.path)
            .parse()
            .map_err(|_| NetError::Refused)?;
        if !allowed(&uri) || uri.host() != Some(request.host.name()) {
            return Err(NetError::Refused);
        }
        let mut builder = http::Request::builder()
            .method(match request.method {
                Method::Get => http::Method::GET,
                Method::Post => http::Method::POST,
            })
            .uri(uri);
        for (name, value) in &request.headers {
            builder = builder.header(*name, value.as_str());
        }
        let body = request.body.unwrap_or_default();
        let built = builder.body(body).map_err(|_| NetError::Refused)?;
        let mut response = self.agent.run(built).map_err(net_error)?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            return Err(NetError::Refused);
        }
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(retry_after);
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_vec()
            .map_err(net_error)?;
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> Uri {
        text.parse().unwrap()
    }

    #[test]
    fn only_the_two_hosts_over_https_on_443_are_allowed() {
        assert!(allowed(&uri("https://api.anthropic.com/v1/messages")));
        assert!(allowed(&uri("https://openrouter.ai/api/v1/models")));
        assert!(allowed(&uri("https://openrouter.ai:443/api/v1/models")));
        for refused in [
            "http://api.anthropic.com/v1/messages",
            "https://api.anthropic.com:8443/v1/messages",
            "https://evil.example/v1/messages",
            "https://api.anthropic.com.evil.example/",
            "https://openrouter.ai.evil.example/",
            "https://user@api.anthropic.com/",
            "https://API.ANTHROPIC.COM/",
            "https://127.0.0.1/",
        ] {
            assert!(!allowed(&uri(refused)), "{refused}");
        }
    }

    #[test]
    fn paths_cannot_change_the_host() {
        assert!(safe_path("/v1/models?limit=1000&after_id=x"));
        for bad in [
            "",
            "v1",
            "//evil.example/",
            "/a#b",
            "/@evil",
            "/a b",
            "/a\\b",
            "/é",
        ] {
            assert!(!safe_path(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_refused_path_is_never_sent() {
        let client = Client::new();
        let request = Request {
            host: Host::Anthropic,
            method: Method::Get,
            path: "//evil.example/".into(),
            headers: Vec::new(),
            body: None,
        };
        assert_eq!(client.send(request).unwrap_err(), NetError::Refused);
    }

    #[test]
    fn an_oversized_body_is_never_sent() {
        let client = Client::new();
        let request = Request {
            host: Host::Anthropic,
            method: Method::Post,
            path: "/v1/messages".into(),
            headers: Vec::new(),
            body: Some(vec![0; MAX_REQUEST_BYTES + 1]),
        };
        assert_eq!(client.send(request).unwrap_err(), NetError::TooLarge);
    }

    #[test]
    fn only_tls_1_3_can_be_agreed() {
        let provider = crypto_provider();
        assert!(!provider.cipher_suites.is_empty());
        assert!(
            provider
                .cipher_suites
                .iter()
                .all(|s| s.version() == &rustls::version::TLS13)
        );
    }

    #[test]
    fn requests_never_print_their_headers() {
        let request = Request {
            host: Host::Anthropic,
            method: Method::Post,
            path: "/v1/messages".into(),
            headers: vec![("x-api-key", Zeroizing::new("sk-ant-TESTONLY".into()))],
            body: Some(b"{}".to_vec()),
        };
        let shown = format!("{request:?}");
        assert!(shown.contains("x-api-key"));
        assert!(!shown.contains("TESTONLY"));
    }

    #[test]
    fn retry_after_is_read_in_seconds() {
        assert_eq!(retry_after(" 30 "), Some(Duration::from_secs(30)));
        assert_eq!(retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }
}
