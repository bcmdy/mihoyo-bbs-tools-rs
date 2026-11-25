mod client;
mod error;

pub use client::{HttpClient, HttpClientBuilder, HttpError, RetryPolicy};
pub use error::{
    HttpErrorCategory, classify_http_error, describe_origin, describe_proxy, redact_url_in_text,
    safe_http_reason, sanitize_reqwest_error,
};
