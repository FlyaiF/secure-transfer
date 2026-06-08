use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use std::ptr;
use std::slice;
use transfer_common::crypto;
use transfer_common::fountain::Decoder;
use transfer_common::protocol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveProgress {
    pub unique_blocks: u32,
    pub decoded_blocks: usize,
    pub total_blocks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveUpdate {
    Ignored,
    InProgress(ReceiveProgress),
    Complete {
        progress: ReceiveProgress,
        file_bytes: Vec<u8>,
    },
}

pub struct ReceiverSession {
    private_key: [u8; 32],
    decoder: Option<Decoder>,
    encrypted_size: Option<u32>,
    unique_blocks: u32,
    completed_file: Option<Vec<u8>>,
}

impl ReceiverSession {
    pub fn new(private_key_base64: &str) -> Result<Self, String> {
        Ok(Self {
            private_key: parse_private_key(private_key_base64)?,
            decoder: None,
            encrypted_size: None,
            unique_blocks: 0,
            completed_file: None,
        })
    }

    pub fn public_key_base64(&self) -> String {
        BASE64.encode(crypto::public_key_from_private(&self.private_key))
    }

    pub fn progress(&self) -> ReceiveProgress {
        ReceiveProgress {
            unique_blocks: self.unique_blocks,
            decoded_blocks: self.decoder.as_ref().map_or(0, Decoder::decoded_count),
            total_blocks: self.decoder.as_ref().map_or(0, Decoder::total_blocks),
        }
    }

    pub fn feed_qr_payload(&mut self, payload: &[u8]) -> Result<ReceiveUpdate, String> {
        if let Some(file_bytes) = &self.completed_file {
            return Ok(ReceiveUpdate::Complete {
                progress: self.progress(),
                file_bytes: file_bytes.clone(),
            });
        }

        let Some(frame) = protocol::decode_frame(payload) else {
            return Ok(ReceiveUpdate::Ignored);
        };

        if self.decoder.is_none() {
            self.decoder = Some(Decoder::new(
                frame.block.total_blocks,
                frame.block_size as usize,
            ));
            self.encrypted_size = Some(frame.encrypted_size);
        }

        if !self.accepts_frame(&frame) {
            return Ok(ReceiveUpdate::Ignored);
        }

        let is_complete = {
            let decoder = self.decoder.as_mut().expect("decoder initialized");
            if decoder.add_block(&frame.block) {
                self.unique_blocks += 1;
            }
            decoder.is_complete()
        };

        let progress = self.progress();
        if is_complete {
            let encrypted_size = self.encrypted_size.expect("encrypted size initialized") as usize;
            let encrypted = self
                .decoder
                .as_ref()
                .expect("decoder initialized")
                .reassemble(encrypted_size)
                .ok_or_else(|| "decoder completed but payload reassembly failed".to_string())?;
            let file_bytes = crypto::decrypt(&encrypted, &self.private_key)?;
            self.completed_file = Some(file_bytes.clone());
            return Ok(ReceiveUpdate::Complete {
                progress,
                file_bytes,
            });
        }

        Ok(ReceiveUpdate::InProgress(progress))
    }

    fn accepts_frame(&self, frame: &protocol::Frame) -> bool {
        let Some(decoder) = &self.decoder else {
            return false;
        };
        self.encrypted_size == Some(frame.encrypted_size)
            && decoder.total_blocks() == frame.block.total_blocks as usize
            && decoder.block_size() == frame.block_size as usize
    }
}

pub fn parse_private_key(private_key_base64: &str) -> Result<[u8; 32], String> {
    let bytes = BASE64
        .decode(private_key_base64.trim())
        .map_err(|_| "invalid base64 private key".to_string())?;
    bytes
        .try_into()
        .map_err(|_| "private key must decode to exactly 32 bytes".to_string())
}

#[repr(C)]
pub struct MobileByteBuffer {
    ptr: *mut u8,
    len: usize,
    cap: usize,
}

impl MobileByteBuffer {
    fn empty() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
            cap: 0,
        }
    }

    fn from_vec(mut bytes: Vec<u8>) -> Self {
        let buffer = Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            cap: bytes.capacity(),
        };
        std::mem::forget(bytes);
        buffer
    }

    fn from_string(value: String) -> Self {
        Self::from_vec(value.into_bytes())
    }
}

#[repr(C)]
pub struct MobileReceiveUpdate {
    kind: u32,
    unique_blocks: u32,
    decoded_blocks: usize,
    total_blocks: usize,
    file: MobileByteBuffer,
    error: MobileByteBuffer,
}

impl MobileReceiveUpdate {
    fn ignored() -> Self {
        Self {
            kind: 0,
            unique_blocks: 0,
            decoded_blocks: 0,
            total_blocks: 0,
            file: MobileByteBuffer::empty(),
            error: MobileByteBuffer::empty(),
        }
    }

    fn with_progress(kind: u32, progress: ReceiveProgress) -> Self {
        Self {
            kind,
            unique_blocks: progress.unique_blocks,
            decoded_blocks: progress.decoded_blocks,
            total_blocks: progress.total_blocks,
            file: MobileByteBuffer::empty(),
            error: MobileByteBuffer::empty(),
        }
    }

    fn complete(progress: ReceiveProgress, file_bytes: Vec<u8>) -> Self {
        Self {
            kind: 2,
            unique_blocks: progress.unique_blocks,
            decoded_blocks: progress.decoded_blocks,
            total_blocks: progress.total_blocks,
            file: MobileByteBuffer::from_vec(file_bytes),
            error: MobileByteBuffer::empty(),
        }
    }

    fn error(message: String) -> Self {
        Self {
            kind: 3,
            unique_blocks: 0,
            decoded_blocks: 0,
            total_blocks: 0,
            file: MobileByteBuffer::empty(),
            error: MobileByteBuffer::from_string(message),
        }
    }
}

/// Free a byte buffer returned by this library.
///
/// # Safety
///
/// `buffer.ptr` must either be null or a pointer previously returned by this
/// library with the paired `buffer.len`.
#[no_mangle]
pub unsafe extern "C" fn transfer_mobile_buffer_free(buffer: MobileByteBuffer) {
    if buffer.ptr.is_null() {
        return;
    }
    drop(Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.cap));
}

/// Create a receive session from a base64 private key.
///
/// Returns null and writes an error buffer on failure.
///
/// # Safety
///
/// `private_key_ptr` must point to `private_key_len` valid bytes. `error_out`
/// may be null; when non-null it must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn transfer_mobile_session_new(
    private_key_ptr: *const u8,
    private_key_len: usize,
    error_out: *mut MobileByteBuffer,
) -> *mut ReceiverSession {
    write_buffer(error_out, MobileByteBuffer::empty());
    match bytes_to_str(private_key_ptr, private_key_len).and_then(ReceiverSession::new) {
        Ok(session) => Box::into_raw(Box::new(session)),
        Err(error) => {
            write_buffer(error_out, MobileByteBuffer::from_string(error));
            ptr::null_mut()
        }
    }
}

/// Free a receive session previously returned by `transfer_mobile_session_new`.
///
/// # Safety
///
/// `session` must be null or a pointer returned by this library that has not
/// already been freed.
#[no_mangle]
pub unsafe extern "C" fn transfer_mobile_session_free(session: *mut ReceiverSession) {
    if session.is_null() {
        return;
    }
    drop(Box::from_raw(session));
}

/// Return the receiver public key as base64 text.
///
/// # Safety
///
/// `session` must be a live session pointer returned by this library.
#[no_mangle]
pub unsafe extern "C" fn transfer_mobile_session_public_key(
    session: *const ReceiverSession,
) -> MobileByteBuffer {
    if session.is_null() {
        return MobileByteBuffer::empty();
    }
    MobileByteBuffer::from_string((*session).public_key_base64())
}

/// Feed one decoded QR payload into the receiver.
///
/// `out_update.kind` values:
/// 0 = ignored, 1 = in progress, 2 = complete, 3 = error.
///
/// # Safety
///
/// `session` must be a live session pointer returned by this library.
/// `payload_ptr` must point to `payload_len` valid bytes. `out_update` must be
/// valid for writes.
#[no_mangle]
pub unsafe extern "C" fn transfer_mobile_session_feed_qr(
    session: *mut ReceiverSession,
    payload_ptr: *const u8,
    payload_len: usize,
    out_update: *mut MobileReceiveUpdate,
) -> bool {
    if out_update.is_null() {
        return false;
    }

    if session.is_null() {
        *out_update = MobileReceiveUpdate::error("receiver session is null".to_string());
        return false;
    }

    let payload = match bytes_from_ptr(payload_ptr, payload_len) {
        Ok(payload) => payload,
        Err(error) => {
            *out_update = MobileReceiveUpdate::error(error);
            return false;
        }
    };

    match (*session).feed_qr_payload(payload) {
        Ok(ReceiveUpdate::Ignored) => {
            *out_update = MobileReceiveUpdate::ignored();
            true
        }
        Ok(ReceiveUpdate::InProgress(progress)) => {
            *out_update = MobileReceiveUpdate::with_progress(1, progress);
            true
        }
        Ok(ReceiveUpdate::Complete {
            progress,
            file_bytes,
        }) => {
            *out_update = MobileReceiveUpdate::complete(progress, file_bytes);
            true
        }
        Err(error) => {
            *out_update = MobileReceiveUpdate::error(error);
            false
        }
    }
}

unsafe fn write_buffer(out: *mut MobileByteBuffer, buffer: MobileByteBuffer) {
    if !out.is_null() {
        *out = buffer;
    }
}

unsafe fn bytes_to_str<'a>(ptr: *const u8, len: usize) -> Result<&'a str, String> {
    let bytes = bytes_from_ptr(ptr, len)?;
    std::str::from_utf8(bytes).map_err(|_| "private key must be UTF-8 text".to_string())
}

unsafe fn bytes_from_ptr<'a>(ptr: *const u8, len: usize) -> Result<&'a [u8], String> {
    if ptr.is_null() {
        return if len == 0 {
            Ok(&[])
        } else {
            Err("input pointer is null".to_string())
        };
    }
    Ok(slice::from_raw_parts(ptr, len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use transfer_common::fountain::{encode_block, split_into_blocks, DEFAULT_BLOCK_SIZE};
    use transfer_common::protocol::encode_frame;

    fn encoded_transfer_frames(data: &[u8], public_key: &[u8; 32]) -> Vec<Vec<u8>> {
        let encrypted = crypto::encrypt(data, public_key).unwrap();
        let blocks = split_into_blocks(&encrypted, DEFAULT_BLOCK_SIZE).unwrap();
        (0..(blocks.len() * 20) as u32)
            .map(|seed| {
                let block = encode_block(&blocks, seed, DEFAULT_BLOCK_SIZE);
                encode_frame(&block, encrypted.len() as u32).unwrap()
            })
            .collect()
    }

    #[test]
    fn derives_public_key_from_private_key() {
        let (private_key, public_key) = crypto::keygen();
        let session = ReceiverSession::new(&BASE64.encode(private_key)).unwrap();
        assert_eq!(session.public_key_base64(), BASE64.encode(public_key));
    }

    #[test]
    fn rejects_invalid_private_key_text() {
        assert_eq!(
            parse_private_key("not base64").unwrap_err(),
            "invalid base64 private key"
        );
    }

    #[test]
    fn receives_file_from_decoded_qr_payloads() {
        let (private_key, public_key) = crypto::keygen();
        let private_key_base64 = BASE64.encode(private_key);
        let data = b"mobile receiver core round trip";
        let frames = encoded_transfer_frames(data, &public_key);
        let mut session = ReceiverSession::new(&private_key_base64).unwrap();

        let mut completed = None;
        for frame in frames {
            match session.feed_qr_payload(&frame).unwrap() {
                ReceiveUpdate::Complete { file_bytes, .. } => {
                    completed = Some(file_bytes);
                    break;
                }
                ReceiveUpdate::Ignored | ReceiveUpdate::InProgress(_) => {}
            }
        }

        assert_eq!(completed.as_deref(), Some(data.as_slice()));
    }

    #[test]
    fn ignores_non_protocol_payloads() {
        let (private_key, _) = crypto::keygen();
        let mut session = ReceiverSession::new(&BASE64.encode(private_key)).unwrap();
        assert_eq!(
            session.feed_qr_payload(b"plain text qr").unwrap(),
            ReceiveUpdate::Ignored
        );
    }
}
