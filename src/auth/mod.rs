mod cookie;
mod credentials;

pub use cookie::{CookieError, CookieJar};
pub use credentials::{CredentialError, Credentials, SecretString};

pub type AuthError = CredentialError;
