use crate::fountain::EncodedBlock;

/// Magic bytes to identify our protocol frames.
pub const MAGIC: [u8; 2] = [b'V', b'T'];
/// Protocol version.
pub const VERSION: u8 = 1;

/// Header size: magic(2) + version(1) + encrypted_size(4) + block_count(2) + seed(4) = 13 bytes
pub const HEADER_SIZE: usize = 13;

/// Encode a protocol frame from an encoded fountain block.
/// Frame: [magic_2B][version_1B][encrypted_size_4B][block_count_2B][seed_4B][payload]
pub fn encode_frame(block: &EncodedBlock, encrypted_size: u32) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER_SIZE + block.data.len());
    frame.extend_from_slice(&MAGIC);
    frame.push(VERSION);
    frame.extend_from_slice(&encrypted_size.to_be_bytes());
    frame.extend_from_slice(&block.total_blocks.to_be_bytes());
    frame.extend_from_slice(&block.seed.to_be_bytes());
    frame.extend_from_slice(&block.data);
    frame
}

/// Decoded protocol frame.
pub struct Frame {
    pub encrypted_size: u32,
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
    let seed = u32::from_be_bytes(data[9..13].try_into().ok()?);
    let payload = data[HEADER_SIZE..].to_vec();

    Some(Frame {
        encrypted_size,
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
        let frame_data = encode_frame(&block, 12800);
        let frame = decode_frame(&frame_data).unwrap();
        assert_eq!(frame.encrypted_size, 12800);
        assert_eq!(frame.block.seed, 42);
        assert_eq!(frame.block.total_blocks, 100);
        assert_eq!(frame.block.data, vec![0xAB; 128]);
    }
}
