/// Custom types and wrappers for serialization
use serde::{Deserialize, Serialize};
use solana_sdk::keccak::Hash;

/// Serializable wrapper for Keccak Hash
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SerializableHash(pub Hash);

impl SerializableHash {
    pub fn new(hash: Hash) -> Self {
        Self(hash)
    }

    pub fn default_hash() -> Self {
        Self(Hash::default())
    }

    pub fn inner(&self) -> &Hash {
        &self.0
    }
}

impl From<Hash> for SerializableHash {
    fn from(hash: Hash) -> Self {
        Self(hash)
    }
}

impl From<SerializableHash> for Hash {
    fn from(hash: SerializableHash) -> Self {
        hash.0
    }
}

impl Serialize for SerializableHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(self.0.as_ref())
    }
}

impl<'de> Deserialize<'de> for SerializableHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes: Vec<u8> = Vec::deserialize(deserializer)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("Invalid hash length"));
        }
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&bytes);
        Ok(SerializableHash(Hash::new(&hash_bytes)))
    }
}

impl Default for SerializableHash {
    fn default() -> Self {
        Self(Hash::default())
    }
}

impl std::fmt::Display for SerializableHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
