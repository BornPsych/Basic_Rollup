use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Serializable hash type using SHA256
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Hash([u8; 32]);

impl Hash {
    pub fn new(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_string(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_string(s: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(s)?;
        if bytes.len() != 32 {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&bytes);
        Ok(Self(hash_bytes))
    }
}

impl Default for Hash {
    fn default() -> Self {
        Self([0u8; 32])
    }
}

impl std::fmt::Display for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

/// Hasher for creating hashes
pub struct Hasher {
    hasher: Sha256,
}

impl Hasher {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    pub fn finalize(self) -> Hash {
        let result = self.hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&result);
        Hash(bytes)
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_creation() {
        let data = b"test data";
        let hash1 = Hash::new(data);
        let hash2 = Hash::new(data);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_string_conversion() {
        let hash = Hash::new(b"test");
        let s = hash.to_string();
        let hash2 = Hash::from_string(&s).unwrap();
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_hasher() {
        let mut hasher = Hasher::new();
        hasher.update(b"hello");
        hasher.update(b"world");
        let _hash = hasher.finalize();
    }
}
