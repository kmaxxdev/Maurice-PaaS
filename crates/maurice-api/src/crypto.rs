//! Encryption at rest for secret values. One master key per panel, kept in
//! `/etc/maurice/maurice.env` as `MAURICE_MASTER_KEY` (hex). Values are sealed
//! with XChaCha20-Poly1305; the 24-byte nonce is prepended to the ciphertext.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use std::io::Read;

#[derive(Clone)]
pub struct MasterKey {
    cipher: XChaCha20Poly1305,
}

fn random_bytes(buf: &mut [u8]) -> anyhow::Result<()> {
    std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
    Ok(())
}

impl MasterKey {
    /// Parse a 64-hex-character key.
    pub fn from_hex(hex_key: &str) -> anyhow::Result<Self> {
        let bytes = hex::decode(hex_key.trim())?;
        anyhow::ensure!(
            bytes.len() == 32,
            "master key must be 32 bytes (64 hex chars)"
        );
        Ok(Self {
            cipher: XChaCha20Poly1305::new_from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("bad key length"))?,
        })
    }

    pub fn generate_hex() -> String {
        let mut k = [0u8; 32];
        random_bytes(&mut k).expect("urandom");
        hex::encode(k)
    }

    pub fn seal(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0u8; 24];
        random_bytes(&mut nonce)?;
        let xn = XNonce::try_from(&nonce[..]).map_err(|_| anyhow::anyhow!("nonce"))?;
        let ct = self
            .cipher
            .encrypt(&xn, plaintext)
            .map_err(|_| anyhow::anyhow!("encrypt"))?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        Ok(out)
    }

    pub fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(sealed.len() > 24, "sealed value too short");
        let (nonce, ct) = sealed.split_at(24);
        let xn = XNonce::try_from(nonce).map_err(|_| anyhow::anyhow!("nonce"))?;
        self.cipher
            .decrypt(&xn, ct)
            .map_err(|_| anyhow::anyhow!("decrypt failed: wrong key or corrupt value"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let k = MasterKey::from_hex(&MasterKey::generate_hex()).unwrap();
        let sealed = k.seal(b"hunter2").unwrap();
        assert_ne!(&sealed[24..], b"hunter2");
        assert_eq!(k.open(&sealed).unwrap(), b"hunter2");
        let other = MasterKey::from_hex(&MasterKey::generate_hex()).unwrap();
        assert!(other.open(&sealed).is_err());
    }
}
