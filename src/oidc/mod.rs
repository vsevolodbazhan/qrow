//! OpenID Connect sign-in for connections. A browser sign-in gives an
//! identity and tokens. Connections send the access token to Kyuubi in the
//! SASL PLAIN password field, over TLS only.
pub mod flow;
pub mod http;
pub mod jwt;
mod service;

pub use service::{EXPIRY_MARGIN, Service, Status};

/// What the user can do about a sign-in failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The stored tokens cannot give an access token. A browser sign-in can.
    SignInRequired,
    /// The provider could not be reached. A retry can succeed.
    Network,
    /// The user cancelled the browser sign-in.
    Cancelled,
    Other,
}

/// A sign-in failure with its recovery. The message never contains tokens.
#[derive(Debug)]
pub struct SignInError {
    pub failure: Failure,
    message: String,
}

impl SignInError {
    pub fn new(failure: Failure, message: impl Into<String>) -> Self {
        Self {
            failure,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SignInError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for SignInError {}

/// The recovery of an error, from the first [`SignInError`] in its chain.
pub fn failure(error: &anyhow::Error) -> Failure {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<SignInError>())
        .map_or(Failure::Other, |error| error.failure)
}
