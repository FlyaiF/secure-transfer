use crate::fountain::EncodedBlock;

/// Magic bytes to identify our protocol frames.
pub const MAGIC: [u8; 2] = [b'V', b'T'];
/// Protocol version.
pub const VERSION: u8 = 2;

/// Header size: magic(2) + version(1) + encrypted_size(4) + block_count(2) + block_size(2) + seed(4) = 15 bytes
pub const HEADER_SIZE: usize = 15;

/// Encode a protocol frame from an encoded fountain block.
/// Frame: [magic_2B][version_1B][encrypted_size_4B][block_count_2B][block_size_2B][seed_4B][payload]
///
/// The header's `block_size` field is derived from `block.data.len()` so the encode/decode
/// round-trip is self-consistent. Returns an error if the payload exceeds `u16::MAX` bytes.
pub fn encode_frame(block: &EncodedBlock, encrypted_size: u32) -> Result<Vec<u8>, String> {
    let block_size: u16 = block.data.len().try_into().map_err(|_| {
        format!(
            "block payload {} bytes exceeds u16::MAX ({})",
            block.data.len(),
            u16::MAX
        )
    })?;
    let mut frame = Vec::with_capacity(HEADER_SIZE + block.data.len());
    frame.extend_from_slice(&MAGIC);
    frame.push(VERSION);
    frame.extend_from_slice(&encrypted_size.to_be_bytes());
    frame.extend_from_slice(&block.total_blocks.to_be_bytes());
    frame.extend_from_slice(&block_size.to_be_bytes());
    frame.extend_from_slice(&block.seed.to_be_bytes());
    frame.extend_from_slice(&block.data);
    Ok(frame)
}

/// Decoded protocol frame.
pub struct Frame {
    pub encrypted_size: u32,
    pub block_size: u16,
    pub block: EncodedBlock,
}

/// Decode a protocol frame. Returns None if magic/version mismatch.
pub fn decode_frame(data: &[u8]) -> Option<Frame> {
    if data.len() < HEADER_SIZE {
        return None;
    }
    if data[0..2] != MAGIC {
        return None;
    }
    if data[2] != VERSION {
        return None;
    }

    let encrypted_size = u32::from_be_bytes(data[3..7].try_into().ok()?);
    let total_blocks = u16::from_be_bytes(data[7..9].try_into().ok()?);
    let block_size = u16::from_be_bytes(data[9..11].try_into().ok()?);
    let seed = u32::from_be_bytes(data[11..15].try_into().ok()?);
    let payload = data[HEADER_SIZE..].to_vec();

    if payload.len() != block_size as usize {
        return None;
    }

    Some(Frame {
        encrypted_size,
        block_size,
        block: EncodedBlock {
            seed,
            total_blocks,
            data: payload,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_roundtrip() {
        let block = EncodedBlock {
            seed: 42,
            total_blocks: 100,
            data: vec![0xAB; 128],
        };
        let frame_data = encode_frame(&block, 12800).unwrap();
        let frame = decode_frame(&frame_data).unwrap();
        assert_eq!(frame.encrypted_size, 12800);
        assert_eq!(frame.block_size, 128);
        assert_eq!(frame.block.seed, 42);
        assert_eq!(frame.block.total_blocks, 100);
        assert_eq!(frame.block.data, vec![0xAB; 128]);
    }

    #[test]
    fn test_frame_roundtrip_large_block() {
        let block = EncodedBlock {
            seed: 7,
            total_blocks: 5,
            data: vec![0xCD; 1024],
        };
        let frame_data = encode_frame(&block, 5000).unwrap();
        let frame = decode_frame(&frame_data).unwrap();
        assert_eq!(frame.block_size, 1024);
        assert_eq!(frame.block.data.len(), 1024);
    }

    #[test]
    fn test_encode_frame_rejects_oversized_payload() {
        let block = EncodedBlock {
            seed: 0,
            total_blocks: 1,
            data: vec![0u8; u16::MAX as usize + 1],
        };
        assert!(encode_frame(&block, 0).is_err());
    }
}
