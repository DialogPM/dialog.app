//! Symmetric cryptography and key derivation used by the core.
//!
//! Key hierarchy (see docs/API.md, section 2):
//!
//! ```text
//! BIP39 mnemonic (24 words) ── entropy = account_secret (32 bytes)
//!     ├─ HKDF "identity"  → Signal identity key pair (never changes for an account)
//!     ├─ HKDF "recovery"  → Ed25519 key pair proving seed ownership to the server
//!     ├─ HKDF "vault"     → AES-256-GCM key for the encrypted server vault
//!     └─ HKDF "profile"   → profile key shared with contacts (encrypts name/about/avatar)
//!
//! password ── Argon2id(salt, m, t, p) → 64 bytes = auth_key (to server) || wrap_key (local only)
//!     wrapped_account_key = AES-256-GCM(wrap_key, account_secret)
//! ```

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use libsignal_protocol::{IdentityKey, IdentityKeyPair, PrivateKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{CoreError, ErrorCode, Result};

type HmacSha256 = Hmac<Sha256>;
type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

const HKDF_SALT: &[u8] = b"ChatApp key derivation v1";
const WRAP_AAD: &[u8] = b"chatapp-account-key-v1";
pub const RECOVERY_SIGNATURE_PREFIX: &[u8] = b"chatapp-recover-v1:";

pub fn b64(data: impl AsRef<[u8]>) -> String {
    STANDARD.encode(data)
}

pub fn unb64(data: &str) -> Result<Vec<u8>> {
    Ok(STANDARD.decode(data.trim())?)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    rand::rng().fill_bytes(&mut out);
    out
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

// ---------------------------------------------------------------------------
// Device names (encrypted for the account's own devices)
// ---------------------------------------------------------------------------

const DEVICE_NAME_AAD: &[u8] = b"chatapp-device-name-v1";

/// Encrypts a device name (padded, so its length is hidden).
pub fn encrypt_device_name(key: &[u8; 32], name: &str) -> Result<Vec<u8>> {
    aead_seal(key, &pad(name.as_bytes()), DEVICE_NAME_AAD)
}

pub fn decrypt_device_name(key: &[u8; 32], data: &[u8]) -> Result<String> {
    let plain = aead_open(key, data, DEVICE_NAME_AAD)?;
    String::from_utf8(unpad(&plain)?.to_vec()).map_err(|_| CoreError::internal("device name is not UTF-8"))
}

// ---------------------------------------------------------------------------
// Registration proof-of-work (server-rs/src/api/security.rs)
// ---------------------------------------------------------------------------

const POW_CONTEXT: &[u8] = b"chatapp-register-pow-v1";

fn pow_leading_zero_bits(challenge: &[u8], nonce: u64) -> u32 {
    let mut h = Sha256::new();
    h.update(POW_CONTEXT);
    h.update(challenge);
    h.update(nonce.to_be_bytes());
    let mut n = 0;
    for b in h.finalize() {
        if b != 0 {
            return n + b.leading_zeros();
        }
        n += 8;
    }
    n
}

/// Finds a nonce such that SHA-256(context || challenge || nonce) starts with `bits` zero bits
/// (about 2^bits hashes).
pub fn solve_pow(challenge: &[u8], bits: u32) -> u64 {
    (0u64..).find(|n| pow_leading_zero_bits(challenge, *n) >= bits).unwrap_or(0)
}

fn hkdf32(ikm: &[u8], info: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), ikm);
    let mut out = [0u8; 32];
    hk.expand(info.as_bytes(), &mut out).expect("32 bytes is a valid HKDF length");
    out
}

/// Group invite links: the token the server sees (hashed) and the key of the preview blob,
/// both derived from the link's secret.
pub fn invite_keys(secret: &[u8; 16]) -> ([u8; 32], [u8; 32]) {
    (hkdf32(secret, "group-invite-token"), hkdf32(secret, "group-invite-blob"))
}

/// Constant-time equality of two byte strings.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Proof that `account` knows a group's invite link secret, bound to the group and account
/// (the server only sees a hash of the link token, so it cannot produce this).
pub fn join_proof(secret: &[u8; 16], group_id: &str, account: &str) -> [u8; 32] {
    let key = hkdf32(secret, "group-join-proof");
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&key).expect("hmac key");
    mac.update(group_id.as_bytes());
    mac.update(b":");
    mac.update(account.as_bytes());
    mac.finalize().into_bytes().into()
}

/// Sealed sender: the key that lets contacts (who know our profile key) deliver messages to
/// us without identifying themselves to the server.
pub fn unidentified_access_key(profile_key: &[u8; 32]) -> [u8; 16] {
    let full = hkdf32(profile_key, "unidentified-access");
    let mut out = [0u8; 16];
    out.copy_from_slice(&full[..16]);
    out
}

// ---------------------------------------------------------------------------
// Mnemonic & account keys
// ---------------------------------------------------------------------------

/// Generates a fresh 24-word English BIP39 mnemonic and returns (words, entropy).
pub fn generate_mnemonic() -> (Vec<String>, AccountSecret) {
    let entropy = random_bytes::<32>();
    let mnemonic = bip39::Mnemonic::from_entropy(&entropy).expect("32 bytes is valid entropy");
    let words = mnemonic.words().map(|w| w.to_string()).collect();
    (words, AccountSecret(entropy))
}

/// The 24 words encoding an account secret.
pub fn mnemonic_words(secret: &AccountSecret) -> Vec<String> {
    let mnemonic = bip39::Mnemonic::from_entropy(&secret.0).expect("32 bytes is valid entropy");
    mnemonic.words().map(|w| w.to_string()).collect()
}

/// Parses a mnemonic typed by the user (tolerates extra whitespace and case).
pub fn parse_mnemonic(words: &[String]) -> Result<AccountSecret> {
    let phrase = words
        .iter()
        .flat_map(|w| w.split_whitespace())
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    let mnemonic = bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &phrase)
        .map_err(|e| CoreError::new(ErrorCode::InvalidMnemonic, e.to_string()))?;
    let entropy = mnemonic.to_entropy();
    if entropy.len() != 32 {
        return Err(CoreError::new(ErrorCode::InvalidMnemonic, "expected 24 words"));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&entropy);
    Ok(AccountSecret(out))
}

/// The 32-byte root secret of an account (the mnemonic entropy).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AccountSecret(pub [u8; 32]);

/// All keys derived from the account secret.
#[derive(Clone)]
pub struct AccountKeys {
    pub identity: IdentityKeyPair,
    pub recovery: ed25519_dalek::SigningKey,
    pub vault_key: [u8; 32],
    pub profile_key: [u8; 32],
    /// Encrypts device names stored on the server (only the account's devices can read them).
    pub device_name_key: [u8; 32],
}

impl AccountKeys {
    pub fn derive(secret: &AccountSecret) -> Result<Self> {
        let identity_bytes = hkdf32(&secret.0, "chatapp/identity");
        let private = PrivateKey::deserialize(&identity_bytes)?;
        let public = private.public_key()?;
        let identity = IdentityKeyPair::new(IdentityKey::new(public), private);
        let recovery = ed25519_dalek::SigningKey::from_bytes(&hkdf32(&secret.0, "chatapp/recovery"));
        Ok(Self {
            identity,
            recovery,
            vault_key: hkdf32(&secret.0, "chatapp/vault"),
            profile_key: hkdf32(&secret.0, "chatapp/profile"),
            device_name_key: hkdf32(&secret.0, "chatapp/device-name"),
        })
    }

    pub fn recovery_public_key(&self) -> [u8; 32] {
        self.recovery.verifying_key().to_bytes()
    }

    pub fn sign_recovery_challenge(&self, challenge: &[u8]) -> [u8; 64] {
        use ed25519_dalek::Signer;
        let mut msg = RECOVERY_SIGNATURE_PREFIX.to_vec();
        msg.extend_from_slice(challenge);
        self.recovery.sign(&msg).to_bytes()
    }
}

// ---------------------------------------------------------------------------
// Password KDF
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub alg: String,
    pub salt: String,
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

/// Lowest Argon2id cost accepted (64 MiB, 3 passes). The parameters come from the server at
/// login: without a floor a malicious server could hand out cheap ones and get an auth key it
/// can brute-force far faster.
pub const MIN_KDF_M: u32 = 64 * 1024;
pub const MIN_KDF_T: u32 = 3;

/// Refuses parameters that are too weak (offline guessing) or absurd (memory DoS).
pub fn check_kdf(kdf: &KdfParams) -> Result<()> {
    if kdf.alg != "argon2id" {
        return Err(CoreError::invalid(format!("unsupported kdf {}", kdf.alg)));
    }
    if kdf.m > 1024 * 1024 || kdf.t > 20 || kdf.p > 8 || kdf.p < 1 {
        return Err(CoreError::invalid("unsafe kdf parameters"));
    }
    if kdf.m < MIN_KDF_M || kdf.t < MIN_KDF_T {
        return Err(CoreError::invalid("the server asked for a password hash that is too weak"));
    }
    if unb64(&kdf.salt).map(|s| s.len()).unwrap_or(0) < 16 {
        return Err(CoreError::invalid("invalid kdf salt"));
    }
    Ok(())
}

impl KdfParams {
    pub fn new_random() -> Self {
        Self { alg: "argon2id".into(), salt: b64(random_bytes::<16>()), m: 65536, t: 3, p: 1 }
    }
}

pub struct PasswordKeys {
    pub auth_key: [u8; 32],
    pub wrap_key: [u8; 32],
}

impl Drop for PasswordKeys {
    fn drop(&mut self) {
        self.auth_key.zeroize();
        self.wrap_key.zeroize();
    }
}

/// Runs Argon2id. This is intentionally slow (~0.5 s); call it off the UI thread.
pub fn derive_password_keys(password: &str, kdf: &KdfParams) -> Result<PasswordKeys> {
    if kdf.alg != "argon2id" {
        return Err(CoreError::invalid(format!("unsupported kdf {}", kdf.alg)));
    }
    check_kdf(kdf)?;
    let salt = unb64(&kdf.salt)?;
    let params = argon2::Params::new(kdf.m, kdf.t, kdf.p, Some(64))
        .map_err(|e| CoreError::internal(format!("argon2 params: {e}")))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 64];
    argon
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .map_err(|e| CoreError::internal(format!("argon2: {e}")))?;
    let mut keys = PasswordKeys { auth_key: [0; 32], wrap_key: [0; 32] };
    keys.auth_key.copy_from_slice(&out[..32]);
    keys.wrap_key.copy_from_slice(&out[32..]);
    out.zeroize();
    Ok(keys)
}

/// Key that wraps the local database key. Same Argon2id cost as the account password, but
/// with its own random salt (kept only on this device) and a domain prefix, so it shares
/// nothing with the keys derived for the server.
pub fn derive_local_db_key(password: &str, kdf: &KdfParams) -> Result<[u8; 32]> {
    if kdf.alg != "argon2id" || kdf.m > 1024 * 1024 || kdf.t > 20 || kdf.p > 8 || kdf.m < 8 * 1024 {
        return Err(CoreError::invalid("unsupported local kdf parameters"));
    }
    let salt = unb64(&kdf.salt)?;
    let params = argon2::Params::new(kdf.m, kdf.t, kdf.p, Some(32))
        .map_err(|e| CoreError::internal(format!("argon2 params: {e}")))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut input = b"chatapp/local-db\0".to_vec();
    input.extend_from_slice(password.as_bytes());
    let mut out = [0u8; 32];
    let r = argon.hash_password_into(&input, &salt, &mut out);
    input.zeroize();
    r.map_err(|e| CoreError::internal(format!("argon2: {e}")))?;
    Ok(out)
}

pub fn wrap_account_secret(wrap_key: &[u8; 32], secret: &AccountSecret) -> Result<Vec<u8>> {
    aead_seal(wrap_key, &secret.0, WRAP_AAD)
}

pub fn unwrap_account_secret(wrap_key: &[u8; 32], wrapped: &[u8]) -> Result<AccountSecret> {
    let plain = aead_open(wrap_key, wrapped, WRAP_AAD)
        .map_err(|_| CoreError::new(ErrorCode::InvalidPassword, "cannot decrypt account key"))?;
    if plain.len() != 32 {
        return Err(CoreError::internal("wrapped account key has wrong length"));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&plain);
    Ok(AccountSecret(out))
}

// ---------------------------------------------------------------------------
// AEAD (AES-256-GCM, random 96-bit nonce prepended)
// ---------------------------------------------------------------------------

pub fn aead_seal(key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let nonce = random_bytes::<12>();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad })
        .map_err(|_| CoreError::internal("encryption failed"))?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn aead_open(key: &[u8; 32], data: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 12 + 16 {
        return Err(CoreError::internal("ciphertext too short"));
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    cipher
        .decrypt(Nonce::from_slice(&data[..12]), Payload { msg: &data[12..], aad })
        .map_err(|_| CoreError::internal("decryption failed"))
}

pub fn key32(bytes: &[u8]) -> Result<[u8; 32]> {
    bytes.try_into().map_err(|_| CoreError::invalid("expected a 32-byte key"))
}

// ---------------------------------------------------------------------------
// Attachments: AES-256-CBC + HMAC-SHA256 (Signal style)
// ciphertext = IV(16) || AES-CBC-PKCS7(plaintext) || HMAC(iv || ct)(32)
// digest     = SHA-256(ciphertext)
// ---------------------------------------------------------------------------

pub struct EncryptedAttachment {
    /// 64 bytes: AES key (32) || HMAC key (32)
    pub key: [u8; 64],
    pub digest: [u8; 32],
    pub ciphertext: Vec<u8>,
}

pub fn encrypt_attachment(plaintext: &[u8]) -> EncryptedAttachment {
    let key = random_bytes::<64>();
    let iv = random_bytes::<16>();
    let ct = Aes256CbcEnc::new_from_slices(&key[..32], &iv)
        .expect("valid key/iv sizes")
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext);
    let mut out = Vec::with_capacity(16 + ct.len() + 32);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&ct);
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&key[32..]).expect("any key size");
    mac.update(&out);
    out.extend_from_slice(&mac.finalize().into_bytes());
    let digest = sha256(&out);
    EncryptedAttachment { key, digest, ciphertext: out }
}

/// Size an attachment is padded to before encryption (Signal's buckets: steps of 5%, at least
/// 541 bytes), so the server does not learn exact file sizes. Never above `cap` unless the
/// file itself is.
pub fn padded_attachment_size(size: usize, cap: usize) -> usize {
    let bucket = if size <= 541 {
        541
    } else {
        1.05f64.powf(((size as f64).ln() / 1.05f64.ln()).ceil()).floor() as usize
    };
    // ln/powf rounding: make sure the bucket covers the file.
    let bucket = bucket.max(size);
    bucket.min(cap.max(size))
}

/// Encrypts a message attachment padded with zeros to its size bucket (the receiver cuts the
/// padding off using the plaintext size from the encrypted message).
pub fn encrypt_attachment_padded(plaintext: &[u8], cap: usize) -> EncryptedAttachment {
    let mut padded = Vec::with_capacity(padded_attachment_size(plaintext.len(), cap));
    padded.extend_from_slice(plaintext);
    padded.resize(padded_attachment_size(plaintext.len(), cap), 0);
    encrypt_attachment(&padded)
}

pub fn decrypt_attachment(key: &[u8], digest: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    if key.len() != 64 {
        return Err(CoreError::invalid("attachment key must be 64 bytes"));
    }
    if data.len() < 16 + 16 + 32 {
        return Err(CoreError::internal("attachment too short"));
    }
    if !bool::from(subtle::ConstantTimeEq::ct_eq(&sha256(data)[..], digest)) {
        return Err(CoreError::internal("attachment digest mismatch"));
    }
    let (body, tag) = data.split_at(data.len() - 32);
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&key[32..]).expect("any key size");
    mac.update(body);
    mac.verify_slice(tag).map_err(|_| CoreError::internal("attachment MAC mismatch"))?;
    let (iv, ct) = body.split_at(16);
    Aes256CbcDec::new_from_slices(&key[..32], iv)
        .expect("valid key/iv sizes")
        .decrypt_padded_vec_mut::<Pkcs7>(ct)
        .map_err(|_| CoreError::internal("attachment padding error"))
}

// ---------------------------------------------------------------------------
// Message padding (hides exact plaintext length), Signal style: 0x80 then zeros.
// ---------------------------------------------------------------------------

const PAD_BLOCK: usize = 160;

pub fn pad(plaintext: &[u8]) -> Vec<u8> {
    let len = plaintext.len() + 1;
    let padded = len.div_ceil(PAD_BLOCK) * PAD_BLOCK;
    let mut out = Vec::with_capacity(padded);
    out.extend_from_slice(plaintext);
    out.push(0x80);
    out.resize(padded, 0);
    out
}

pub fn unpad(data: &[u8]) -> Result<&[u8]> {
    let end = data.iter().rposition(|&b| b != 0).ok_or_else(|| CoreError::internal("bad padding"))?;
    if data[end] != 0x80 {
        return Err(CoreError::internal("bad padding"));
    }
    Ok(&data[..end])
}

/// Salted hash of an account number, kept in the desktop app's profile list so a hidden
/// profile can be found by its number without storing the number. Argon2id (19 MiB, 2 passes)
/// with a per-profile random salt and a domain prefix: guessing the 16 digits back costs one
/// Argon2 run per candidate, per profile.
pub fn profile_number_hash(number: &str, salt: &[u8]) -> Result<[u8; 32]> {
    if salt.len() < 16 {
        return Err(CoreError::invalid("invalid salt"));
    }
    let params = argon2::Params::new(19 * 1024, 2, 1, Some(32)).map_err(|e| CoreError::internal(format!("argon2 params: {e}")))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut input = b"chatapp/profile-number\0".to_vec();
    input.extend_from_slice(number.as_bytes());
    let mut out = [0u8; 32];
    argon
        .hash_password_into(&input, salt, &mut out)
        .map_err(|e| CoreError::internal(format!("argon2: {e}")))?;
    Ok(out)
}

/// Opaque vault record id: HMAC(vault_key, logical id), so the server cannot see
/// which conversation or contact a record belongs to.
pub fn vault_record_id(vault_key: &[u8; 32], logical_id: &str) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(vault_key).expect("any key size");
    mac.update(logical_id.as_bytes());
    hex::encode(&mac.finalize().into_bytes()[..24])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_roundtrip_and_deterministic_keys() {
        let (words, secret) = generate_mnemonic();
        assert_eq!(words.len(), 24);
        let parsed = parse_mnemonic(&[words.join("  ").to_uppercase()]).unwrap();
        assert_eq!(parsed.0, secret.0);
        let a = AccountKeys::derive(&secret).unwrap();
        let b = AccountKeys::derive(&parsed).unwrap();
        assert_eq!(a.identity.identity_key(), b.identity.identity_key());
        assert_eq!(a.vault_key, b.vault_key);
        assert_ne!(a.vault_key, a.profile_key);
        assert_ne!(a.device_name_key, a.vault_key);
    }

    #[test]
    fn device_names_and_pow() {
        let key = [3u8; 32];
        let enc = encrypt_device_name(&key, "Ana's laptop").unwrap();
        assert_eq!(decrypt_device_name(&key, &enc).unwrap(), "Ana's laptop");
        assert_eq!(enc.len(), encrypt_device_name(&key, "x").unwrap().len());
        assert!(decrypt_device_name(&[4u8; 32], &enc).is_err());
        let nonce = solve_pow(&[9u8; 32], 10);
        assert!(pow_leading_zero_bits(&[9u8; 32], nonce) >= 10);
    }

    #[test]
    fn profile_number_hash_is_salted() {
        let (a, b) = ([1u8; 16], [2u8; 16]);
        let h = profile_number_hash("1234567812345678", &a).unwrap();
        assert_eq!(h, profile_number_hash("1234567812345678", &a).unwrap());
        assert_ne!(h, profile_number_hash("1234567812345678", &b).unwrap());
        assert_ne!(h, profile_number_hash("1234567812345679", &a).unwrap());
        assert!(profile_number_hash("1", &[0u8; 8]).is_err());
    }

    #[test]
    fn password_wrap_roundtrip() {
        let (_, secret) = generate_mnemonic();
        let kdf = KdfParams::new_random();
        // A server cannot make us use a cheap hash.
        assert!(derive_password_keys("x", &KdfParams { m: 8192, t: 1, ..KdfParams::new_random() }).is_err());
        assert!(derive_password_keys("x", &KdfParams { t: 2, ..KdfParams::new_random() }).is_err());
        let keys = derive_password_keys("correct horse", &kdf).unwrap();
        let wrapped = wrap_account_secret(&keys.wrap_key, &secret).unwrap();
        let back = unwrap_account_secret(&keys.wrap_key, &wrapped).unwrap();
        assert_eq!(back.0, secret.0);
        let wrong = derive_password_keys("wrong", &kdf).unwrap();
        assert!(unwrap_account_secret(&wrong.wrap_key, &wrapped).is_err());
    }

    #[test]
    fn attachment_padding_hides_the_size() {
        let cap = 100 * 1024 * 1024;
        for size in [0usize, 1, 540, 541, 542, 10_000, 1_234_567, 99 * 1024 * 1024] {
            let p = padded_attachment_size(size, cap);
            assert!(p >= size && p >= 541.min(cap), "{size} -> {p}");
            assert!(p as f64 <= (size as f64 * 1.051).max(541.0), "{size} -> {p}");
        }
        // Neighbouring sizes share a bucket.
        let b = padded_attachment_size(1_000_000, cap);
        assert_eq!(padded_attachment_size(b, cap), b);
        assert_eq!(padded_attachment_size(b - 1000, cap), b);
        // Never above the server limit (unless the file is).
        assert_eq!(padded_attachment_size(cap - 10, cap), cap);
        let enc = encrypt_attachment_padded(b"hello", cap);
        let plain = decrypt_attachment(&enc.key, &enc.digest, &enc.ciphertext).unwrap();
        assert_eq!(plain.len(), 541);
        assert_eq!(&plain[..5], b"hello");
    }

    #[test]
    fn attachment_roundtrip() {
        let data = vec![7u8; 1000];
        let enc = encrypt_attachment(&data);
        assert_eq!(decrypt_attachment(&enc.key, &enc.digest, &enc.ciphertext).unwrap(), data);
        let mut bad = enc.ciphertext.clone();
        bad[20] ^= 1;
        assert!(decrypt_attachment(&enc.key, &enc.digest, &bad).is_err());
    }

    #[test]
    fn padding_roundtrip() {
        for n in [0usize, 1, 159, 160, 500] {
            let v = vec![1u8; n];
            let p = pad(&v);
            assert_eq!(p.len() % PAD_BLOCK, 0);
            assert_eq!(unpad(&p).unwrap(), &v[..]);
        }
    }
}
