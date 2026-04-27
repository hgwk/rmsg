use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

const SALT: &[u8] = b"relay-chat";
const NONCE_SIZE: usize = 12;

pub struct KeyPair {
    pub pk: [u8; 32],
    pub sk: StaticSecret,
}

impl KeyPair {
    pub fn generate() -> Self {
        let sk = StaticSecret::random_from_rng(OsRng);
        let pk = PublicKey::from(&sk).to_bytes();
        Self { pk, sk }
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.sk.to_bytes()
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let sk = StaticSecret::from(*bytes);
        let pk = PublicKey::from(&sk).to_bytes();
        Self { pk, sk }
    }

    pub fn load_or_generate(path: &std::path::Path) -> Result<Self, std::io::Error> {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                return Ok(Self::from_bytes(&arr));
            }
        }
        let kp = Self::generate();
        std::fs::write(path, kp.sk.to_bytes())?;
        Ok(kp)
    }
}

pub struct CryptoSession {
    aes: Option<Aes256Gcm>,
    room_id: String,
}

impl CryptoSession {
    pub fn new(room_id: String) -> Self {
        Self {
            aes: None,
            room_id,
        }
    }

    pub fn start(&mut self, my_sk: &StaticSecret, peer_pk: &[u8; 32]) -> Result<(), CryptoError> {
        let peer_public = PublicKey::from(*peer_pk);
        let shared_secret = my_sk.diffie_hellman(&peer_public);

        let hkdf = Hkdf::<Sha256>::new(Some(SALT), shared_secret.as_bytes());
        let info = self.room_id.as_bytes();
        let mut okm = [0u8; 32];
        hkdf.expand(info, &mut okm)
            .map_err(|_| CryptoError::KeyDerivation)?;

        let aes = Aes256Gcm::new_from_slice(&okm)
            .map_err(|_| CryptoError::KeyDerivation)?;
        self.aes = Some(aes);
        Ok(())
    }

    pub fn encrypt(&self, plain: &[u8], seq: u64) -> Result<Encrypted, CryptoError> {
        let aes = self.aes.as_ref().ok_or(CryptoError::NotReady)?;

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let aad = format!("{}:{}", self.room_id, seq);
        let payload = aes
            .encrypt(nonce, aes_gcm::aead::Payload {
                msg: plain,
                aad: aad.as_bytes(),
            })
            .map_err(|_| CryptoError::Encryption)?;

        Ok(Encrypted {
            ct: payload,
            nonce: nonce_bytes,
        })
    }

    pub fn decrypt(&self, ct: &[u8], nonce: &[u8; NONCE_SIZE], seq: u64) -> Result<Vec<u8>, CryptoError> {
        let aes = self.aes.as_ref().ok_or(CryptoError::NotReady)?;

        let nonce_slice = Nonce::from_slice(nonce);
        let aad = format!("{}:{}", self.room_id, seq);
        let plain = aes
            .decrypt(nonce_slice, aes_gcm::aead::Payload {
                msg: ct,
                aad: aad.as_bytes(),
            })
            .map_err(|_| CryptoError::Decryption)?;

        Ok(plain)
    }

    #[allow(dead_code)]
    pub fn is_ready(&self) -> bool {
        self.aes.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct Encrypted {
    pub ct: Vec<u8>,
    pub nonce: [u8; NONCE_SIZE],
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("session not ready")]
    NotReady,
    #[error("key derivation failed")]
    KeyDerivation,
    #[error("encryption failed")]
    Encryption,
    #[error("decryption failed")]
    Decryption,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let alice = KeyPair::generate();
        let bob = KeyPair::generate();

        let mut alice_sess = CryptoSession::new("test-room".into());
        let mut bob_sess = CryptoSession::new("test-room".into());

        alice_sess.start(&alice.sk, &bob.pk).unwrap();
        bob_sess.start(&bob.sk, &alice.pk).unwrap();

        let plain = b"hello rmsg!";
        let enc = alice_sess.encrypt(plain, 1).unwrap();
        let dec = bob_sess.decrypt(&enc.ct, &enc.nonce, 1).unwrap();
        assert_eq!(dec, plain);
    }

    #[test]
    fn test_different_rooms_no_cross_decrypt() {
        let alice = KeyPair::generate();
        let bob = KeyPair::generate();

        let mut alice_sess = CryptoSession::new("room-a".into());
        let bob_sess = CryptoSession::new("room-b".into());
        let mut bob_sess_a = CryptoSession::new("room-a".into());

        alice_sess.start(&alice.sk, &bob.pk).unwrap();
        bob_sess_a.start(&bob.sk, &alice.pk).unwrap();

        let enc = alice_sess.encrypt(b"secret", 1).unwrap();
        assert!(bob_sess.decrypt(&enc.ct, &enc.nonce, 1).is_err());
        assert!(bob_sess_a.decrypt(&enc.ct, &enc.nonce, 1).is_ok());
    }

    #[test]
    fn test_keypair_persistence() {
        let tmp = std::env::temp_dir().join("rmsg-test-key");
        let kp1 = KeyPair::load_or_generate(&tmp).unwrap();
        let kp2 = KeyPair::load_or_generate(&tmp).unwrap();
        assert_eq!(kp1.pk, kp2.pk);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_not_ready_error() {
        let sess = CryptoSession::new("room".into());
        assert!(sess.encrypt(b"hi", 1).is_err());
        assert!(sess.decrypt(b"x", &[0u8; 12], 1).is_err());
    }
}
