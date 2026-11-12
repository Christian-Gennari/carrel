//! Small, dependency-free hashes used by the journal and file ids.

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a, 64 bit. Not cryptographic: it only guards journal records against
/// truncated or corrupted writes, and gives stable ids to journals.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_update(FNV_OFFSET, bytes)
}

pub fn fnv1a64_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
