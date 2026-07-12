mod client;
mod cookie;
mod credentials;

pub use client::{AuthClient, AuthEndpoints, MihoyoApiEnvelope, RefreshOnce};
pub use cookie::{CookieError, CookieJar};
pub use credentials::{CredentialError, Credentials, SecretString};

pub type AuthError = CredentialError;
