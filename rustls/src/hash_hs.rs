use crate::msgs::codec::Codec;
use crate::msgs::handshake::HandshakeMessagePayload;
use crate::msgs::message::{Message, MessagePayload};
use ring::digest;
use std::mem;

pub struct HandshakeHashBuffer {
    buffer: Vec<u8>,
    client_auth_enabled: bool,
}

impl HandshakeHashBuffer {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            client_auth_enabled: false,
        }
    }

    /// We might be doing client auth, so need to keep a full
    /// log of the handshake.
    pub fn set_client_auth_enabled(&mut self) {
        self.client_auth_enabled = true;
    }

    /// Hash/buffer a handshake message.
    pub fn add_message(&mut self, m: &Message) {
        match m.payload {
            MessagePayload::Handshake(ref hs) => {
                self.buffer.extend_from_slice(&hs.get_encoding());
            }
            _ => {}
        }
    }

    /// Hash or buffer a byte slice.
    #[cfg(test)]
    fn update_raw(&mut self, buf: &[u8]) {
        self.buffer.extend_from_slice(buf);
    }

    /// Get the hash value if we were to hash `extra` too.
    pub fn get_hash_given(&self, hash: &'static digest::Algorithm, extra: &[u8]) -> digest::Digest {
        let mut ctx = digest::Context::new(hash);
        ctx.update(&self.buffer);
        ctx.update(extra);
        ctx.finish()
    }

    /// We now know what hash function the verify_data will use.
    pub fn start_hash(mut self, alg: &'static digest::Algorithm) -> HandshakeHash {
        let mut ctx = digest::Context::new(alg);
        ctx.update(&self.buffer);

        // Discard buffer if we don't need it now.
        if !self.client_auth_enabled {
            self.buffer.drain(..);
        }

        HandshakeHash {
            ctx,
            client_auth_enabled: self.client_auth_enabled,
            buffer: self.buffer,
        }
    }
}

/// This deals with keeping a running hash of the handshake
/// payloads.  This is computed by buffering initially.  Once
/// we know what hash function we need to use we switch to
/// incremental hashing.
///
/// For client auth, we also need to buffer all the messages.
/// This is disabled in cases where client auth is not possible.
pub struct HandshakeHash {
    /// None before we know what hash function we're using
    ctx: digest::Context,

    /// true if we need to keep all messages
    client_auth_enabled: bool,

    /// buffer for client-auth.
    buffer: Vec<u8>,
}

impl HandshakeHash {
    /// We decided not to do client auth after all, so discard
    /// the transcript.
    pub fn abandon_client_auth(&mut self) {
        self.client_auth_enabled = false;
        self.buffer.drain(..);
    }

    /// Hash/buffer a handshake message.
    pub fn add_message(&mut self, m: &Message) -> &mut HandshakeHash {
        match m.payload {
            MessagePayload::Handshake(ref hs) => {
                let buf = hs.get_encoding();
                self.update_raw(&buf);
            }
            _ => {}
        };
        self
    }

    /// Hash or buffer a byte slice.
    fn update_raw(&mut self, buf: &[u8]) -> &mut Self {
        self.ctx.update(buf);

        if self.client_auth_enabled {
            self.buffer.extend_from_slice(buf);
        }

        self
    }

    /// Get the hash value if we were to hash `extra` too,
    /// using hash function `hash`.
    pub fn get_hash_given(&self, extra: &[u8]) -> digest::Digest {
        let mut ctx = self.ctx.clone();
        ctx.update(extra);
        ctx.finish()
    }

    pub fn into_hrr_buffer(self) -> HandshakeHashBuffer {
        let old_hash = self.ctx.finish();
        let old_handshake_hash_msg =
            HandshakeMessagePayload::build_handshake_hash(old_hash.as_ref());

        HandshakeHashBuffer {
            client_auth_enabled: self.client_auth_enabled,
            buffer: old_handshake_hash_msg.get_encoding(),
        }
    }

    /// Take the current hash value, and encapsulate it in a
    /// 'handshake_hash' handshake message.  Start this hash
    /// again, with that message at the front.
    pub fn rollup_for_hrr(&mut self) {
        let ctx = &mut self.ctx;

        let old_ctx = mem::replace(ctx, digest::Context::new(ctx.algorithm()));
        let old_hash = old_ctx.finish();
        let old_handshake_hash_msg =
            HandshakeMessagePayload::build_handshake_hash(old_hash.as_ref());

        self.update_raw(&old_handshake_hash_msg.get_encoding());
    }

    /// Get the current hash value.
    pub fn get_current_hash(&self) -> digest::Digest {
        self.ctx.clone().finish()
    }

    /// Takes this object's buffer containing all handshake messages
    /// so far.  This method only works once; it resets the buffer
    /// to empty.
    pub fn take_handshake_buf(&mut self) -> Vec<u8> {
        debug_assert!(self.client_auth_enabled);
        mem::replace(&mut self.buffer, Vec::new())
    }

    /// The digest algorithm
    pub fn algorithm(&self) -> &'static digest::Algorithm {
        self.ctx.algorithm()
    }
}

#[cfg(test)]
mod test {
    use super::HandshakeHashBuffer;
    use ring::digest;

    #[test]
    fn hashes_correctly() {
        let mut hhb = HandshakeHashBuffer::new();
        hhb.update_raw(b"hello");
        assert_eq!(hhb.buffer.len(), 5);
        let mut hh = hhb.start_hash(&digest::SHA256);
        assert_eq!(hh.buffer.len(), 0);
        hh.update_raw(b"world");
        let h = hh.get_current_hash();
        let h = h.as_ref();
        assert_eq!(h[0], 0x93);
        assert_eq!(h[1], 0x6a);
        assert_eq!(h[2], 0x18);
        assert_eq!(h[3], 0x5c);
    }

    #[test]
    fn buffers_correctly() {
        let mut hhb = HandshakeHashBuffer::new();
        hhb.set_client_auth_enabled();
        hhb.update_raw(b"hello");
        assert_eq!(hhb.buffer.len(), 5);
        let mut hh = hhb.start_hash(&digest::SHA256);
        assert_eq!(hh.buffer.len(), 5);
        hh.update_raw(b"world");
        assert_eq!(hh.buffer.len(), 10);
        let h = hh.get_current_hash();
        let h = h.as_ref();
        assert_eq!(h[0], 0x93);
        assert_eq!(h[1], 0x6a);
        assert_eq!(h[2], 0x18);
        assert_eq!(h[3], 0x5c);
        let buf = hh.take_handshake_buf();
        assert_eq!(b"helloworld".to_vec(), buf);
    }

    #[test]
    fn abandon() {
        let mut hhb = HandshakeHashBuffer::new();
        hhb.set_client_auth_enabled();
        hhb.update_raw(b"hello");
        assert_eq!(hhb.buffer.len(), 5);
        let mut hh = hhb.start_hash(&digest::SHA256);
        assert_eq!(hh.buffer.len(), 5);
        hh.abandon_client_auth();
        assert_eq!(hh.buffer.len(), 0);
        hh.update_raw(b"world");
        assert_eq!(hh.buffer.len(), 0);
        let h = hh.get_current_hash();
        let h = h.as_ref();
        assert_eq!(h[0], 0x93);
        assert_eq!(h[1], 0x6a);
        assert_eq!(h[2], 0x18);
        assert_eq!(h[3], 0x5c);
    }
}
