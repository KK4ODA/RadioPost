// SPDX-License-Identifier: MIT OR Apache-2.0
//! AGWPE's frames (SV2AGW, *AGWPE TCP/IP API*): a 36-byte header, then its data.
//!
//! | Bytes | Field |
//! |---|---|
//! | 0 | The TNC's port, 0 for the first |
//! | 1–3 | Reserved, 0 |
//! | 4 | The kind, a letter (`X` register a callsign, `C` connect, `D` data, `d` disconnect…) |
//! | 5 | Reserved |
//! | 6 | The PID, `0xF0` for AX.25 data |
//! | 7 | Reserved |
//! | 8–17 | `call_from`, ASCII, padded with NUL |
//! | 18–27 | `call_to`, ASCII, padded with NUL |
//! | 28–31 | The data's length, unsigned, little-endian |
//! | 32–35 | Reserved |

/// The header's length.
pub const HEADER: usize = 36;
/// The longest data a frame may carry here: far more than any AGWPE frame holds.
pub const MAX_DATA: usize = 1 << 16;
/// AX.25's PID for data that carries no layer 3 (I and UI frames).
pub const PID_NONE: u8 = 0xF0;

/// One frame, either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The TNC's port.
    pub port: u8,
    /// The kind, a letter.
    pub kind: u8,
    /// The PID.
    pub pid: u8,
    /// `call_from`.
    pub from: String,
    /// `call_to`.
    pub to: String,
    /// The data.
    pub data: Vec<u8>,
}

impl Frame {
    /// A frame of `kind` on `port`, with no callsigns and no data.
    #[must_use]
    pub fn new(port: u8, kind: u8) -> Frame {
        Frame {
            port,
            kind,
            pid: 0,
            from: String::new(),
            to: String::new(),
            data: Vec::new(),
        }
    }

    /// With `call_from` and `call_to`.
    #[must_use]
    pub fn calls(mut self, from: &str, to: &str) -> Frame {
        from.clone_into(&mut self.from);
        to.clone_into(&mut self.to);
        self
    }

    /// With data, and AX.25's PID for it.
    #[must_use]
    pub fn data(mut self, data: &[u8]) -> Frame {
        self.pid = PID_NONE;
        self.data = data.to_vec();
        self
    }

    /// The frame's bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER];
        out[0] = self.port;
        out[4] = self.kind;
        out[6] = self.pid;
        let call = |field: &mut [u8], call: &str| {
            for (slot, b) in field.iter_mut().zip(call.bytes().take(9)) {
                *slot = b;
            }
        };
        call(&mut out[8..18], &self.from);
        call(&mut out[18..28], &self.to);
        let len = u32::try_from(self.data.len()).unwrap_or(u32::MAX);
        out[28..32].copy_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// The data as text, without the NUL and CR AGWPE ends its messages with.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data)
            .trim_end_matches(['\0', '\r', '\n'])
            .to_owned()
    }
}

/// A callsign field: up to its first NUL.
fn call_of(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_owned()
}

/// Frames read from a stream of bytes, in whatever pieces they come.
#[derive(Debug, Default)]
pub struct Reader {
    held: Vec<u8>,
}

impl Reader {
    /// An empty reader.
    #[must_use]
    pub fn new() -> Reader {
        Reader::default()
    }

    /// Bytes read.
    pub fn push(&mut self, bytes: &[u8]) {
        self.held.extend_from_slice(bytes);
    }

    /// The next whole frame, if there is one.
    ///
    /// # Errors
    ///
    /// A header gives a length no frame here carries: the stream is not AGWPE's.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        if self.held.len() < HEADER {
            return Ok(None);
        }
        let mut len = [0u8; 4];
        len.copy_from_slice(&self.held[28..32]);
        let len = usize::try_from(u32::from_le_bytes(len)).unwrap_or(usize::MAX);
        if len > MAX_DATA {
            return Err(format!(
                "a frame of {len} bytes: what was read is not AGWPE's"
            ));
        }
        if self.held.len() < HEADER + len {
            return Ok(None);
        }
        let h = &self.held[..HEADER];
        let frame = Frame {
            port: h[0],
            kind: h[4],
            pid: h[6],
            from: call_of(&h[8..18]),
            to: call_of(&h[18..28]),
            data: self.held[HEADER..HEADER + len].to_vec(),
        };
        self.held.drain(..HEADER + len);
        Ok(Some(frame))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_reads_back_whole_from_any_pieces() {
        let f = Frame::new(1, b'D')
            .calls("KK4ODA", "N1CALL-10")
            .data(b"hello");
        let bytes = f.encode();
        assert_eq!(bytes.len(), HEADER + 5);
        assert_eq!(&bytes[8..15], b"KK4ODA\0");
        assert_eq!(&bytes[28..32], &[5, 0, 0, 0]);
        let mut r = Reader::new();
        for b in &bytes {
            assert_eq!(r.next_frame(), Ok(None));
            r.push(&[*b]);
        }
        assert_eq!(r.next_frame(), Ok(Some(f)));
        assert_eq!(r.next_frame(), Ok(None));
    }

    #[test]
    fn a_length_no_frame_carries_is_refused() {
        let mut bytes = Frame::new(0, b'R').encode();
        bytes[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut r = Reader::new();
        r.push(&bytes);
        assert!(r.next_frame().is_err());
    }

    #[test]
    fn agwpe_messages_read_as_text() {
        let f = Frame::new(0, b'C').data(b"*** CONNECTED With Station N1CALL-10\r\0");
        assert_eq!(f.text(), "*** CONNECTED With Station N1CALL-10");
    }
}
