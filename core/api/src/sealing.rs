// SPDX-License-Identifier: MIT OR Apache-2.0
//! A file sealed to a public key and opened only with its private half: how a post office's
//! backup leaves the server for its operator's computer by itself (wishlist W38), with nothing
//! secret kept on the server.
//!
//! The operator's computer keeps an X25519 key pair ([`KeyPair`]); the post office knows only the
//! public half. Sealing draws a fresh key pair for the file, agrees a secret with the recipient's
//! public key, derives a key from it with HKDF-SHA256 (salted with both public keys), and seals
//! the file with ChaCha20-Poly1305. What is written: the magic line, the recipient's key ID
//! (8 bytes), the fresh public key (32), the nonce (12), then the sealed bytes and their tag. The
//! header is bound to the seal, so a file changed anywhere does not open. Whoever holds the server,
//! or a copy of the file, learns nothing that opens it.

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, Salt};
use x25519_dalek::{PublicKey, StaticSecret};

/// The first bytes of a sealed file. Version 1 was a backup sealed with a passphrase.
pub const MAGIC: &[u8] = b"RADIOPOST-BACKUP-2\n";

/// What HKDF's expansion is told: these files, this version.
const INFO: &[u8] = b"radiopost sealed file 1";

/// The bytes of a key ID.
const ID_BYTES: usize = 8;

/// The header after the magic line: key ID, the file's own public key, the nonce.
const HEADER: usize = ID_BYTES + 32 + 12;

/// A key ID: the first eight bytes of SHA-256 of the public key, in hex. Short enough to say, long
/// enough that two of one operator's computers never share one.
#[must_use]
pub fn key_id(public: &[u8; 32]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, public);
    radiopost::cbor::hex(&digest.as_ref()[..ID_BYTES])
}

/// An X25519 key pair: a computer's, to open what is sealed to it.
pub struct KeyPair {
    secret: StaticSecret,
    public: PublicKey,
}

impl KeyPair {
    /// A new key pair from the system's random numbers.
    ///
    /// # Errors
    ///
    /// No random numbers.
    pub fn generate() -> Result<KeyPair, String> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| e.to_string())?;
        Ok(KeyPair::from_seed(seed))
    }

    fn from_seed(seed: [u8; 32]) -> KeyPair {
        let secret = StaticSecret::from(seed);
        let public = PublicKey::from(&secret);
        KeyPair { secret, public }
    }

    /// A key pair kept as its private key in hex (what [`KeyPair::secret_hex`] wrote).
    ///
    /// # Errors
    ///
    /// Not 32 bytes in hex.
    pub fn from_hex(text: &str) -> Result<KeyPair, String> {
        let seed: [u8; 32] = unhex(text.trim())
            .and_then(|b| b.try_into().ok())
            .ok_or("not a key: 32 bytes in hex")?;
        Ok(KeyPair::from_seed(seed))
    }

    /// The private key in hex, to keep on the computer and nowhere else.
    #[must_use]
    pub fn secret_hex(&self) -> String {
        radiopost::cbor::hex(self.secret.as_bytes())
    }

    /// The public key, which the post office keeps.
    #[must_use]
    pub fn public(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    /// Its key ID ([`key_id`]).
    #[must_use]
    pub fn id(&self) -> String {
        key_id(&self.public())
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A public key given in hex.
///
/// # Errors
///
/// Not 32 bytes in hex.
pub fn public_from_hex(text: &str) -> Result<[u8; 32], String> {
    unhex(text.trim())
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| "a public key is 32 bytes in hex".to_owned())
}

/// The key that seals between `ours` and `theirs`, salted with the file's public key and the
/// recipient's.
fn file_key(
    ours: &StaticSecret,
    theirs: &PublicKey,
    file_public: &[u8; 32],
    recipient: &[u8; 32],
) -> Result<LessSafeKey, String> {
    let shared = ours.diffie_hellman(theirs);
    // a public key of low order agrees on a secret anyone knows
    if !shared.was_contributory() {
        return Err("not a usable public key".into());
    }
    let mut salt = file_public.to_vec();
    salt.extend_from_slice(recipient);
    let key: UnboundKey = Salt::new(HKDF_SHA256, &salt)
        .extract(shared.as_bytes())
        .expand(&[INFO], &CHACHA20_POLY1305)
        .map_err(|_| "the key could not be made".to_owned())?
        .into();
    Ok(LessSafeKey::new(key))
}

/// `plain` sealed to the computer whose public key is `to`.
///
/// # Errors
///
/// No random numbers, or `to` is not a usable public key.
pub fn seal(plain: &[u8], to: &[u8; 32]) -> Result<Vec<u8>, String> {
    let file = KeyPair::generate()?;
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
    let file_public = file.public();
    let key = file_key(&file.secret, &PublicKey::from(*to), &file_public, to)?;
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&id_bytes(to));
    out.extend_from_slice(&file_public);
    out.extend_from_slice(&nonce);
    let mut data = plain.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(out.as_slice()),
        &mut data,
    )
    .map_err(|_| "the file could not be sealed".to_owned())?;
    out.extend_from_slice(&data);
    Ok(out)
}

fn id_bytes(public: &[u8; 32]) -> [u8; ID_BYTES] {
    let digest = ring::digest::digest(&ring::digest::SHA256, public);
    let mut id = [0u8; ID_BYTES];
    id.copy_from_slice(&digest.as_ref()[..ID_BYTES]);
    id
}

/// Whether `file` is sealed this way.
#[must_use]
pub fn is_sealed(file: &[u8]) -> bool {
    file.starts_with(MAGIC)
}

/// The key ID a sealed file names: which computer it opens on.
#[must_use]
pub fn recipient(file: &[u8]) -> Option<String> {
    let rest = file.strip_prefix(MAGIC)?;
    Some(radiopost::cbor::hex(rest.get(..ID_BYTES)?))
}

/// What `file` holds, opened with `keys`.
///
/// # Errors
///
/// Not a sealed file, sealed to another key, cut short, or changed since it was sealed.
pub fn open(file: &[u8], keys: &KeyPair) -> Result<Vec<u8>, String> {
    let rest = file
        .strip_prefix(MAGIC)
        .ok_or("not a sealed RadioPost file")?;
    if rest.len() < HEADER + 16 {
        return Err("the file is cut short".into());
    }
    let id = radiopost::cbor::hex(&rest[..ID_BYTES]);
    if id != keys.id() {
        return Err(format!(
            "this file is sealed to the key {id}, and this computer's is {}",
            keys.id()
        ));
    }
    let file_public: [u8; 32] = rest[ID_BYTES..ID_BYTES + 32]
        .try_into()
        .map_err(|_| "the file is cut short")?;
    let nonce = Nonce::try_assume_unique_for_key(&rest[ID_BYTES + 32..HEADER])
        .map_err(|_| "a bad nonce")?;
    let key = file_key(
        &keys.secret,
        &PublicKey::from(file_public),
        &file_public,
        &keys.public(),
    )?;
    let header = &file[..MAGIC.len() + HEADER];
    let mut data = rest[HEADER..].to_vec();
    let opened = key
        .open_in_place(nonce, Aad::from(header), &mut data)
        .map_err(|_| "the file has changed since it was sealed".to_owned())?;
    Ok(opened.to_vec())
}

#[cfg(test)]
mod tests {
    use super::{KeyPair, MAGIC, is_sealed, key_id, open, public_from_hex, recipient, seal};

    /// Sealed to one computer, a file opens there alone, says whose it is, and does not open
    /// once changed anywhere, its header included; the key pair survives being kept in hex.
    #[test]
    fn a_file_opens_only_on_the_computer_it_was_sealed_to() {
        let laptop = KeyPair::generate().expect("a key pair");
        let other = KeyPair::generate().expect("a key pair");
        let plain = b"the post office's backup".repeat(100);
        let sealed = seal(&plain, &laptop.public()).expect("sealed");
        assert!(is_sealed(&sealed) && sealed.starts_with(MAGIC));
        assert_eq!(recipient(&sealed), Some(laptop.id()));
        assert_eq!(laptop.id(), key_id(&laptop.public()));
        assert_eq!(laptop.id().len(), 16);
        assert_eq!(open(&sealed, &laptop).expect("opened"), plain);
        let refused = open(&sealed, &other).expect_err("another computer's key");
        assert!(refused.contains(&laptop.id()), "{refused}");
        // kept in hex and read back, it still opens
        let kept = KeyPair::from_hex(&laptop.secret_hex()).expect("read back");
        assert_eq!(kept.public(), laptop.public());
        assert_eq!(open(&sealed, &kept).expect("opened"), plain);
        // a bit changed in the body, the nonce, the file's public key or the key ID
        for at in [
            sealed.len() - 1,
            MAGIC.len() + 50,
            MAGIC.len() + 20,
            MAGIC.len() + 1,
        ] {
            let mut bent = sealed.clone();
            bent[at] ^= 1;
            assert!(open(&bent, &laptop).is_err(), "changed at {at}");
        }
        assert!(open(&sealed[..sealed.len() - 20], &laptop).is_err());
        assert!(open(&sealed[..MAGIC.len() + 30], &laptop).is_err());
        // each sealing is fresh: the same file twice is two different files
        assert_ne!(sealed, seal(&plain, &laptop.public()).expect("sealed"));
        // a public key of low order is refused rather than sealing to a secret anyone knows
        assert!(seal(&plain, &[0u8; 32]).is_err());
        assert!(public_from_hex("abcd").is_err());
        assert_eq!(
            public_from_hex(&radiopost::cbor::hex(&laptop.public())).expect("hex"),
            laptop.public()
        );
    }
}
