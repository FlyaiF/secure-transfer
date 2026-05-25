use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;

/// Default block size in bytes for fountain encoding. Used when no value is specified.
pub const DEFAULT_BLOCK_SIZE: usize = 128;

/// Robust Soliton distribution parameters.
const C_SOLITON: f64 = 0.1;
const DELTA_SOLITON: f64 = 0.5;

/// An encoded fountain block.
#[derive(Clone, Debug)]
pub struct EncodedBlock {
    /// Seed used to determine which source blocks are XORed.
    pub seed: u32,
    /// Total number of source blocks.
    pub total_blocks: u16,
    /// The XORed payload data.
    pub data: Vec<u8>,
}

/// Maximum supported payload size for a given block size (u16::MAX blocks).
pub fn max_payload_size(block_size: usize) -> usize {
    u16::MAX as usize * block_size
}

/// Split data into source blocks of `block_size`, padding the last block with zeros.
/// Returns an error if `block_size` is zero or the data exceeds the block-count limit.
pub fn split_into_blocks(data: &[u8], block_size: usize) -> Result<Vec<Vec<u8>>, String> {
    if block_size == 0 {
        return Err("block_size must be greater than 0".to_string());
    }
    let block_count = data.len().div_ceil(block_size);
    if block_count > u16::MAX as usize {
        return Err(format!(
            "payload too large: {} bytes ({} blocks) exceeds maximum {} bytes ({} blocks) at block_size={}",
            data.len(),
            block_count,
            max_payload_size(block_size),
            u16::MAX,
            block_size,
        ));
    }
    let mut blocks = Vec::new();
    for chunk in data.chunks(block_size) {
        let mut block = chunk.to_vec();
        block.resize(block_size, 0);
        blocks.push(block);
    }
    if blocks.is_empty() {
        blocks.push(vec![0u8; block_size]);
    }
    Ok(blocks)
}

/// Generate a fountain-encoded block from source blocks using the given seed.
/// Caller must ensure every block in `source_blocks` has length `block_size`.
pub fn encode_block(source_blocks: &[Vec<u8>], seed: u32, block_size: usize) -> EncodedBlock {
    let k = source_blocks.len();
    let indices = select_indices(seed, k);

    let mut data = vec![0u8; block_size];
    for &idx in &indices {
        xor_into(&mut data, &source_blocks[idx]);
    }

    EncodedBlock {
        seed,
        total_blocks: k as u16,
        data,
    }
}

/// Fountain decoder using belief propagation (peeling).
pub struct Decoder {
    k: usize,
    block_size: usize,
    decoded: Vec<Option<Vec<u8>>>,
    /// Buffered encoded blocks not yet fully resolved: (remaining_indices, data)
    buffer: Vec<(HashSet<usize>, Vec<u8>)>,
    decoded_count: usize,
    seen_seeds: HashSet<u32>,
}

impl Decoder {
    pub fn new(total_blocks: u16, block_size: usize) -> Self {
        let k = total_blocks as usize;
        Decoder {
            k,
            block_size,
            decoded: vec![None; k],
            buffer: Vec::new(),
            decoded_count: 0,
            seen_seeds: HashSet::new(),
        }
    }

    /// Number of source blocks successfully decoded so far.
    pub fn decoded_count(&self) -> usize {
        self.decoded_count
    }

    /// Total number of source blocks needed.
    pub fn total_blocks(&self) -> usize {
        self.k
    }

    /// Block size in bytes this decoder was initialized with.
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Returns true if all source blocks have been decoded.
    pub fn is_complete(&self) -> bool {
        self.decoded_count == self.k
    }

    /// Add an encoded block and attempt to decode. Returns true if new source blocks were decoded.
    /// Blocks whose payload length doesn't match the decoder's `block_size` are rejected.
    pub fn add_block(&mut self, block: &EncodedBlock) -> bool {
        if self.is_complete() {
            return false;
        }
        if block.data.len() != self.block_size {
            return false;
        }
        if !self.seen_seeds.insert(block.seed) {
            return false; // duplicate
        }

        let indices = select_indices(block.seed, self.k);
        let remaining: HashSet<usize> = indices
            .into_iter()
            .filter(|i| self.decoded[*i].is_none())
            .collect();
        let mut data = block.data.clone();

        // XOR out already-decoded blocks
        for &idx in &select_indices(block.seed, self.k) {
            if let Some(ref decoded_data) = self.decoded[idx] {
                if !remaining.contains(&idx) {
                    xor_into(&mut data, decoded_data);
                }
            }
        }

        if remaining.len() == 1 {
            let idx = *remaining.iter().next().unwrap();
            self.resolve(idx, data);
            true
        } else if remaining.is_empty() {
            false // redundant
        } else {
            self.buffer.push((remaining, data));
            false
        }
    }

    fn resolve(&mut self, idx: usize, data: Vec<u8>) {
        if self.decoded[idx].is_some() {
            return;
        }
        self.decoded[idx] = Some(data);
        self.decoded_count += 1;

        // Propagate: reduce all buffered blocks that reference this index
        let mut newly_resolved = Vec::new();
        for entry in &mut self.buffer {
            if entry.0.remove(&idx) {
                xor_into(&mut entry.1, self.decoded[idx].as_ref().unwrap());
                if entry.0.len() == 1 {
                    let next_idx = *entry.0.iter().next().unwrap();
                    newly_resolved.push((next_idx, entry.1.clone()));
                    entry.0.clear(); // mark as consumed
                }
            }
        }

        // Remove consumed entries
        self.buffer.retain(|e| !e.0.is_empty());

        // Recursively resolve
        for (next_idx, next_data) in newly_resolved {
            self.resolve(next_idx, next_data);
        }
    }

    /// Reassemble the decoded data. Returns None if not all blocks are decoded.
    pub fn reassemble(&self, original_size: usize) -> Option<Vec<u8>> {
        if !self.is_complete() {
            return None;
        }
        let mut result = Vec::with_capacity(self.k * self.block_size);
        for block in &self.decoded {
            result.extend_from_slice(block.as_ref().unwrap());
        }
        result.truncate(original_size);
        Some(result)
    }
}

/// Select which source block indices to XOR for a given seed, using Robust Soliton distribution.
fn select_indices(seed: u32, k: usize) -> Vec<usize> {
    let mut rng = StdRng::seed_from_u64(seed as u64);
    let degree = sample_robust_soliton(&mut rng, k).min(k);

    // Pick `degree` unique random indices
    let mut indices = HashSet::with_capacity(degree);
    while indices.len() < degree {
        indices.insert(rng.gen_range(0..k));
    }
    indices.into_iter().collect()
}

/// Sample from the Robust Soliton distribution.
fn sample_robust_soliton(rng: &mut StdRng, k: usize) -> usize {
    let k_f = k as f64;
    let r = C_SOLITON * (k_f / DELTA_SOLITON).ln() * k_f.sqrt();

    // Build CDF of ideal + robust soliton
    let mut cdf = Vec::with_capacity(k);
    let mut total = 0.0;

    for d in 1..=k {
        // Ideal soliton
        let rho = if d == 1 {
            1.0 / k_f
        } else {
            1.0 / (d as f64 * (d as f64 - 1.0))
        };

        // Robust addition (tau)
        let tau = if d == 1 {
            r / k_f
        } else if d < (k_f / r).ceil() as usize {
            r / (d as f64 * k_f)
        } else if d == (k_f / r).ceil() as usize {
            r * (r / DELTA_SOLITON).ln() / k_f
        } else {
            0.0
        };

        total += rho + tau;
        cdf.push(total);
    }

    // Normalize CDF
    for v in &mut cdf {
        *v /= total;
    }

    // Sample
    let u: f64 = rng.gen();
    for (i, &c) in cdf.iter().enumerate() {
        if u <= c {
            return i + 1; // degree is 1-indexed
        }
    }
    1 // fallback
}

/// XOR src into dst in-place.
fn xor_into(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d ^= *s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fountain_roundtrip() {
        let data = b"The quick brown fox jumps over the lazy dog. This is a test of the fountain code system with enough data to span multiple blocks for proper testing purposes.";
        let bs = DEFAULT_BLOCK_SIZE;
        let source_blocks = split_into_blocks(data, bs).unwrap();
        let k = source_blocks.len();

        let mut decoder = Decoder::new(k as u16, bs);
        let mut seed = 0u32;

        // Generate encoded blocks until decoding succeeds
        while !decoder.is_complete() {
            let block = encode_block(&source_blocks, seed, bs);
            decoder.add_block(&block);
            seed += 1;
            if seed > (k as u32) * 20 {
                panic!("too many blocks needed, something is wrong");
            }
        }

        let result = decoder.reassemble(data.len()).unwrap();
        assert_eq!(result, data);
        eprintln!(
            "Decoded {} source blocks using {} encoded blocks (overhead: {:.1}%)",
            k,
            seed,
            ((seed as f64 / k as f64) - 1.0) * 100.0
        );
    }

    #[test]
    fn test_fountain_skip_blocks() {
        // Simulate frame drops: only use every other encoded block
        let bs = DEFAULT_BLOCK_SIZE;
        let data = vec![42u8; bs * 10]; // 10 blocks
        let source_blocks = split_into_blocks(&data, bs).unwrap();
        let k = source_blocks.len();

        let mut decoder = Decoder::new(k as u16, bs);
        let mut seed = 0u32;

        while !decoder.is_complete() {
            let block = encode_block(&source_blocks, seed, bs);
            // Only use even seeds (simulate 50% frame drop)
            if seed.is_multiple_of(2) {
                decoder.add_block(&block);
            }
            seed += 1;
            if seed > (k as u32) * 50 {
                panic!("too many blocks needed");
            }
        }

        let result = decoder.reassemble(data.len()).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_fountain_custom_block_size() {
        let bs = 1024;
        let data: Vec<u8> = (0..bs * 4).map(|i| (i % 256) as u8).collect();
        let source_blocks = split_into_blocks(&data, bs).unwrap();
        let k = source_blocks.len();

        let mut decoder = Decoder::new(k as u16, bs);
        let mut seed = 0u32;
        while !decoder.is_complete() {
            let block = encode_block(&source_blocks, seed, bs);
            decoder.add_block(&block);
            seed += 1;
            if seed > (k as u32) * 20 {
                panic!("too many blocks needed");
            }
        }
        assert_eq!(decoder.reassemble(data.len()).unwrap(), data);
    }

    #[test]
    fn test_decoder_rejects_mismatched_block_size() {
        let mut decoder = Decoder::new(4, 128);
        let bad = EncodedBlock {
            seed: 1,
            total_blocks: 4,
            data: vec![0u8; 256],
        };
        assert!(!decoder.add_block(&bad));
    }
}
