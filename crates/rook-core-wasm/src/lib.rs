use core::sync::atomic::{AtomicU64, Ordering};

// Wasm scalar returns avoid sharing a pointer-based memory ABI with the host.
// The host reconstructs each 32-byte digest from four little-endian words.
static HASH_WORDS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Computes both hashes once and caches them for scalar export.
#[unsafe(no_mangle)]
pub extern "C" fn replay(event_count: u32) {
    let hashes = rook_core::replay_synthetic(event_count);
    for (word_index, bytes) in hashes
        .run_hash
        .chunks_exact(8)
        .chain(hashes.output_digest.chunks_exact(8))
        .enumerate()
    {
        let mut word = [0_u8; 8];
        word.copy_from_slice(bytes);
        HASH_WORDS[word_index].store(u64::from_le_bytes(word), Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn hash_word(hash_kind: u32, word_index: u32) -> u64 {
    let index = hash_kind as usize * 4 + word_index as usize;
    HASH_WORDS[index].load(Ordering::Relaxed)
}
