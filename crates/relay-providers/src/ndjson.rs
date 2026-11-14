//! Newline-delimited JSON decoding for Ollama.

use bytes::Bytes;

/// Incremental NDJSON line splitter.
///
/// UTF-8 characters split across chunks are buffered until their line is
/// complete, then decoded.
#[derive(Debug, Default)]
pub struct NdjsonDecoder {
    buffer: Vec<u8>,
}

impl NdjsonDecoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a network chunk and return every complete non-empty line.
    pub fn push(&mut self, chunk: &Bytes) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.is_empty() {
                lines.push(String::from_utf8_lossy(&line).into_owned());
            }
        }
        lines
    }

    /// Flush a trailing line at end of stream.
    pub fn finish(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let mut line = std::mem::take(&mut self.buffer);
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&line).into_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndjson_splits_and_flushes() {
        let mut decoder = NdjsonDecoder::new();
        let lines = decoder.push(&Bytes::from_static(b"{\"a\":1}\n{\"b\":2}\r\n"));
        assert_eq!(lines, vec!["{\"a\":1}", "{\"b\":2}"]);
        assert!(decoder.push(&Bytes::from_static(b"{\"c\":3}")).is_empty());
        assert_eq!(decoder.finish().as_deref(), Some("{\"c\":3}"));
    }

    #[test]
    fn utf8_split_across_chunks_is_preserved() {
        let mut decoder = NdjsonDecoder::new();
        assert!(
            decoder
                .push(&Bytes::from_static(b"{\"x\":\"caf"))
                .is_empty()
        );
        assert!(decoder.push(&Bytes::from_static(&[0xC3])).is_empty());
        let lines = decoder.push(&Bytes::from_static(&[0xA9, b'"', b'}', b'\n']));
        assert_eq!(lines, vec!["{\"x\":\"café\"}"]);
    }
}
