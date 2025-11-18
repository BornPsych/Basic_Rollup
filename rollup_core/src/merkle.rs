use solana_sdk::{hash::hashv, keccak::{Hash, Hasher}};
use std::collections::HashMap;

/// Merkle tree implementation for state root calculation
#[derive(Debug, Clone)]
pub struct MerkleTree {
    leaves: Vec<Hash>,
    nodes: Vec<Vec<Hash>>,
}

impl MerkleTree {
    /// Create a new Merkle tree from a list of leaf hashes
    pub fn new(mut leaves: Vec<Hash>) -> Self {
        if leaves.is_empty() {
            leaves.push(Hash::default());
        }

        // Ensure we have an even number of leaves for pairing
        if leaves.len() % 2 != 0 {
            leaves.push(*leaves.last().unwrap());
        }

        let mut current_level = leaves.clone();
        let mut nodes = vec![leaves];

        // Build the tree bottom-up
        while current_level.len() > 1 {
            let mut next_level = Vec::new();

            for i in (0..current_level.len()).step_by(2) {
                let left = current_level[i];
                let right = if i + 1 < current_level.len() {
                    current_level[i + 1]
                } else {
                    current_level[i]
                };

                let mut hasher = Hasher::default();
                hasher.hash(left.as_ref());
                hasher.hash(right.as_ref());
                let parent = hasher.result();
                next_level.push(parent);
            }

            nodes.push(next_level.clone());
            current_level = next_level;
        }

        MerkleTree { leaves, nodes }
    }

    /// Get the root hash of the Merkle tree
    pub fn root(&self) -> Hash {
        self.nodes.last()
            .and_then(|level| level.first())
            .copied()
            .unwrap_or_default()
    }

    /// Get a Merkle proof for a leaf at the given index
    pub fn get_proof(&self, index: usize) -> Option<Vec<Hash>> {
        if index >= self.leaves.len() {
            return None;
        }

        let mut proof = Vec::new();
        let mut current_index = index;

        for level in &self.nodes[..self.nodes.len() - 1] {
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };

            if sibling_index < level.len() {
                proof.push(level[sibling_index]);
            }

            current_index /= 2;
        }

        Some(proof)
    }

    /// Verify a Merkle proof
    pub fn verify_proof(leaf: Hash, proof: &[Hash], root: Hash) -> bool {
        let mut current_hash = leaf;

        for sibling in proof {
            let mut hasher = Hasher::default();
            // Determine order based on hash comparison
            if current_hash.as_ref() <= sibling.as_ref() {
                hasher.hash(current_hash.as_ref());
                hasher.hash(sibling.as_ref());
            } else {
                hasher.hash(sibling.as_ref());
                hasher.hash(current_hash.as_ref());
            }
            current_hash = hasher.result();
        }

        current_hash == root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree_single_leaf() {
        let leaf = Hash::new(&[1u8; 32]);
        let tree = MerkleTree::new(vec![leaf]);
        assert_eq!(tree.root(), leaf);
    }

    #[test]
    fn test_merkle_tree_multiple_leaves() {
        let leaves = vec![
            Hash::new(&[1u8; 32]),
            Hash::new(&[2u8; 32]),
            Hash::new(&[3u8; 32]),
            Hash::new(&[4u8; 32]),
        ];
        let tree = MerkleTree::new(leaves.clone());

        // Root should be deterministic
        assert_ne!(tree.root(), Hash::default());
    }

    #[test]
    fn test_merkle_proof() {
        let leaves = vec![
            Hash::new(&[1u8; 32]),
            Hash::new(&[2u8; 32]),
            Hash::new(&[3u8; 32]),
            Hash::new(&[4u8; 32]),
        ];
        let tree = MerkleTree::new(leaves.clone());

        let proof = tree.get_proof(0).unwrap();
        assert!(MerkleTree::verify_proof(leaves[0], &proof, tree.root()));
    }
}
