use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use rand::{Rng, seq::SliceRandom};

pub const APP_SALT: &str = "47f15f1b66bee46b816115d8e8e6ebb6";
pub const WEB_SALT: &str = "d9200c846b10886e8c874fc33c8f308b";
pub const BODY_SALT: &str = "t0qEgfub6cvueAPgR5m9aQWWVciEer7v";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DsHeader {
    pub timestamp: u64,
    pub random: String,
    pub checksum: String,
}

impl fmt::Display for DsHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{},{},{}",
            self.timestamp, self.random, self.checksum
        )
    }
}

pub trait Clock {
    fn unix_timestamp(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn unix_timestamp(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn unix_timestamp(&self) -> u64 {
        self.0
    }
}

pub trait DsRandom {
    fn legacy_nonce(&mut self) -> String;
    fn body_nonce(&mut self) -> u32;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadRandom;

impl DsRandom for ThreadRandom {
    fn legacy_nonce(&mut self) -> String {
        let mut alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789".to_vec();
        alphabet.shuffle(&mut rand::rng());
        alphabet.into_iter().take(6).map(char::from).collect()
    }

    fn body_nonce(&mut self) -> u32 {
        rand::rng().random_range(100_001..=200_000)
    }
}

#[derive(Clone, Debug)]
pub struct FixedRandom {
    pub legacy: String,
    pub body: u32,
}

impl FixedRandom {
    pub fn new(legacy: impl Into<String>, body: u32) -> Self {
        Self {
            legacy: legacy.into(),
            body,
        }
    }
}

impl DsRandom for FixedRandom {
    fn legacy_nonce(&mut self) -> String {
        self.legacy.clone()
    }

    fn body_nonce(&mut self) -> u32 {
        self.body
    }
}

#[derive(Clone, Debug)]
pub struct DsSigner<C, R> {
    clock: C,
    random: R,
}

impl<C, R> DsSigner<C, R>
where
    C: Clock,
    R: DsRandom,
{
    pub fn new(clock: C, random: R) -> Self {
        Self { clock, random }
    }

    pub fn sign_app(&mut self) -> DsHeader {
        sign_ds_with(
            APP_SALT,
            self.clock.unix_timestamp(),
            &self.random.legacy_nonce(),
        )
    }

    pub fn sign_web(&mut self) -> DsHeader {
        sign_ds_with(
            WEB_SALT,
            self.clock.unix_timestamp(),
            &self.random.legacy_nonce(),
        )
    }

    pub fn sign_body(&mut self, query: &str, body: &[u8]) -> DsHeader {
        sign_ds2_with(
            BODY_SALT,
            self.clock.unix_timestamp(),
            self.random.body_nonce(),
            query,
            body,
        )
    }
}

pub fn sign_ds_with(salt: &str, timestamp: u64, random: &str) -> DsHeader {
    let input = format!("salt={salt}&t={timestamp}&r={random}");
    DsHeader {
        timestamp,
        random: random.to_owned(),
        checksum: md5_hex(input.as_bytes()),
    }
}

/// `body` 应直接传入随后发送的同一组 UTF-8 JSON 字节，避免序列化差异导致验签失败。
pub fn sign_ds2_with(
    salt: &str,
    timestamp: u64,
    random: u32,
    query: &str,
    body: &[u8],
) -> DsHeader {
    let prefix = format!("salt={salt}&t={timestamp}&r={random}&b=");
    let suffix = format!("&q={query}");
    let mut hasher = Md5::new();
    hasher.update(prefix.as_bytes());
    hasher.update(body);
    hasher.update(suffix.as_bytes());
    DsHeader {
        timestamp,
        random: random.to_string(),
        checksum: format!("{:x}", hasher.finalize()),
    }
}

fn md5_hex(input: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(input);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_web_ds_from_fixed_vector() {
        let header = sign_ds_with(WEB_SALT, 1_700_000_000, "abc123");
        assert_eq!(
            header,
            DsHeader {
                timestamp: 1_700_000_000,
                random: "abc123".to_owned(),
                checksum: "0469c15c8dda31d69eb62a61935c2c26".to_owned(),
            }
        );
        assert_eq!(
            header.to_string(),
            "1700000000,abc123,0469c15c8dda31d69eb62a61935c2c26"
        );
    }

    #[test]
    fn creates_app_ds_from_fixed_vector() {
        let header = sign_ds_with(APP_SALT, 1_700_000_000, "z9x8c7");
        assert_eq!(header.checksum, "f0d53f5ee5be00fafcd94d4d1d6448a0");
    }

    #[test]
    fn creates_body_ds_from_exact_query_and_body_bytes() {
        let header = sign_ds2_with(
            BODY_SALT,
            1_700_000_000,
            123_456,
            "foo=bar",
            br#"{"gids":"2"}"#,
        );
        assert_eq!(
            header.to_string(),
            "1700000000,123456,86864c6c03e06442cbd6df13a84a5028"
        );
    }

    #[test]
    fn injected_clock_and_random_are_deterministic() {
        let mut signer = DsSigner::new(
            FixedClock(1_700_000_000),
            FixedRandom::new("abc123", 100_001),
        );

        assert_eq!(
            signer.sign_web().checksum,
            "0469c15c8dda31d69eb62a61935c2c26"
        );
        assert_eq!(
            signer.sign_body("", b"").checksum,
            "e1357c283b822606e22e90ceb5a8be79"
        );
    }

    #[test]
    fn body_whitespace_is_significant() {
        let compact = sign_ds2_with(BODY_SALT, 1, 100_001, "", br#"{"gids":"2"}"#);
        let spaced = sign_ds2_with(BODY_SALT, 1, 100_001, "", br#"{"gids": "2"}"#);
        assert_ne!(compact.checksum, spaced.checksum);
    }

    #[test]
    fn thread_random_matches_original_ranges() {
        let mut random = ThreadRandom;
        let legacy = random.legacy_nonce();
        let body = random.body_nonce();

        assert_eq!(legacy.len(), 6);
        assert!(
            legacy
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        );
        let mut unique = legacy.bytes().collect::<Vec<_>>();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 6);
        assert!((100_001..=200_000).contains(&body));
    }
}
