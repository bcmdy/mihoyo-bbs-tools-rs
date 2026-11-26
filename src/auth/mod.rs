mod client;
mod cookie;
mod credentials;

pub use client::{
    AuthClient, AuthEndpoints, BackfillOutcome, MihoyoApiEnvelope, PassportDevice,
    PassportTokenTarget, RefreshOnce, backfill_v2_tokens,
};
pub use cookie::{CookieError, CookieJar};
pub use credentials::{CredentialError, Credentials, SecretString};

pub type AuthError = CredentialError;
