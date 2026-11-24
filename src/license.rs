//! The one-off unlock. A licence key is the buyer's email signed with
//! Ed25519 by the licence server; it is checked here offline against the
//! embedded public key, so the app never phones home.
//!
//! Key format: `ATT1.<base64url email>.<base64url signature>`.
//!
//! The unlock adds the file queue and the larger model. While `BUY_URL` is
//! empty there is no paid tier at all: both are open to everyone and no Buy
//! or licence screen shows, so a build can never ship a paywall nobody can
//! pay.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, VerifyingKey};

/// Where "Buy" would send people. Empty in 1.0: no licence server yet.
pub const BUY_URL: &str = "";

/// Public half of the licence server's signing key, as hex.
///
/// PLACEHOLDER - REPLACE BEFORE SELLING. This is the well-known RFC 8032
/// test key, whose private half is public; it only keeps the format honest
/// until a licence server exists. It is harmless while `BUY_URL` is empty.
const PUBLIC_KEY_HEX: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

const PREFIX: &str = "ATT1";

pub fn paid_tier_enabled() -> bool {
    !BUY_URL.is_empty()
}

/// Whether the queue and the larger model are open.
pub fn unlocked(licensed: bool) -> bool {
    licensed || !paid_tier_enabled()
}

fn public_key() -> Option<[u8; 32]> {
    let hex = PUBLIC_KEY_HEX.as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in hex.chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        out[i] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

/// Returns the licensed email if `key` is genuine.
pub fn verify(key: &str) -> Option<String> {
    verify_with(&public_key()?, key)
}

fn verify_with(public_key: &[u8; 32], key: &str) -> Option<String> {
    // Keys get pasted from emails and web pages: tolerate stray whitespace.
    let key: String = key.chars().filter(|c| !c.is_whitespace()).collect();
    let mut parts = key.split('.');
    let (prefix, email, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if prefix != PREFIX || parts.next().is_some() {
        return None;
    }
    let email = URL_SAFE_NO_PAD.decode(email).ok()?;
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    let signature = Signature::from_slice(&signature).ok()?;
    let verifying_key = VerifyingKey::from_bytes(public_key).ok()?;
    verifying_key.verify_strict(&email, &signature).ok()?;
    String::from_utf8(email).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn issue(signing_key: &SigningKey, email: &str) -> String {
        let signature = signing_key.sign(email.as_bytes());
        format!(
            "{PREFIX}.{}.{}",
            URL_SAFE_NO_PAD.encode(email),
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        )
    }

    #[test]
    fn accepts_a_genuine_key_and_returns_the_email() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let public = signing_key.verifying_key().to_bytes();
        let key = issue(&signing_key, "buyer@example.com");
        assert_eq!(verify_with(&public, &key).as_deref(), Some("buyer@example.com"));
        let messy = format!("  {}\n{} ", &key[..20], &key[20..]);
        assert_eq!(verify_with(&public, &messy).as_deref(), Some("buyer@example.com"));
    }

    #[test]
    fn rejects_forged_altered_and_malformed_keys() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let public = signing_key.verifying_key().to_bytes();
        let other = SigningKey::from_bytes(&[8u8; 32]);
        assert_eq!(verify_with(&public, &issue(&other, "a@b.c")), None);

        let key = issue(&signing_key, "a@b.c");
        let signature = key.rsplit('.').next().unwrap();
        let swapped = format!("{PREFIX}.{}.{signature}", URL_SAFE_NO_PAD.encode("z@b.c"));
        assert_eq!(verify_with(&public, &swapped), None);

        // Another app's prefix is not accepted, even with a valid signature.
        let foreign = key.replacen(PREFIX, "MAC1", 1);
        assert_eq!(verify_with(&public, &foreign), None);

        for bad in ["", "ATT1", "ATT1..", "MAC1.YQ.YQ", "not a key", &format!("{key}.x")] {
            assert_eq!(verify_with(&public, bad), None, "{bad}");
        }
    }

    #[test]
    fn embedded_public_key_is_well_formed() {
        let bytes = public_key().expect("64 hex digits");
        assert!(VerifyingKey::from_bytes(&bytes).is_ok());
        assert_eq!(verify("ATT1.YQ.YQ"), None);
    }

    #[test]
    fn everything_is_open_while_there_is_no_buy_url() {
        assert!(!paid_tier_enabled());
        assert!(unlocked(false));
        assert!(unlocked(true));
    }
}
