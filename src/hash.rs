//! FNV-1a-64, in one place.
//!
//! Two things need a stable non-cryptographic hash of some bytes: an image tag
//! (`image::tag_digest`) and an ssh port (`ssh::port`). Both used to roll their
//! own — the port used `DefaultHasher`, which is explicitly not stable across
//! toolchains, so a compiler bump moved every session's port and broke the
//! editor bookmarks pointing at them. One implementation, pinned by vectors.
//!
//! Not for resisting an adversary: the inputs are omh's own recipe text and
//! two public names, and a collision costs at worst a reused layer or a probed
//! port.

/// FNV-1a over the bytes: offset basis, then xor-then-multiply per byte.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical FNV-1a-64 vectors. If these move, every image tag and
    /// every ssh port moves with them.
    #[test]
    fn the_canonical_vectors_hold() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
