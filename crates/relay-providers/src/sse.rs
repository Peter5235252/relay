//! A small, allocation-conscious Server-Sent Events decoder.
//!
//! Providers frame events a little differently, but the wire format is the
//! same. This decoder handles CRLF, comments, multi-line `data:` fields, chunk
//! boundaries that split UTF-8 characters, and a final event without a blank
//! terminator.

use bytes::Bytes;

/// One dispatched SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Value of the `event:` field, when present.
    pub event: Option<String>,
    /// Joined `data:` fields.
    pub data: String,
}

/// Incremental SSE parser.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    event: Option<String>,
    data_lines: Vec<String>,
    saw_field: bool,
}

impl SseDecoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a network chunk and return every completed event.
    pub fn push(&mut self, chunk: &Bytes) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.line(&String::from_utf8_lossy(&line), &mut events);
        }
        events
    }

    /// Flush a trailing event at end of stream.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            self.line(&String::from_utf8_lossy(&line), &mut events);
        }
        self.dispatch(&mut events);
        events
    }

    fn line(&mut self, line: &str, events: &mut Vec<SseEvent>) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            self.dispatch(events);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => {
                self.event = Some(value.to_owned());
                self.saw_field = true;
            }
            "data" => {
                self.data_lines.push(value.to_owned());
                self.saw_field = true;
            }
            // `id`, `retry`, and future fields are irrelevant to stateless
            // streaming; keep scanning without treating them as data.
            _ => {}
        }
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        if !self.saw_field {
            self.event = None;
            self.data_lines.clear();
            return;
        }
        let data = self.data_lines.join("\n");
        if !data.is_empty() {
            events.push(SseEvent {
                event: self.event.take(),
                data,
            });
        }
        self.event = None;
        self.data_lines.clear();
        self.saw_field = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_and_multiline_data() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(&Bytes::from_static(
            b"event: greeting\ndata: hello\ndata: world\n\n: comment\ndata:{\"x\":1}\n\n",
        ));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("greeting"));
        assert_eq!(events[0].data, "hello\nworld");
        assert_eq!(events[1].event, None);
        assert_eq!(events[1].data, "{\"x\":1}");
    }

    #[test]
    fn handles_crlf_and_split_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(&Bytes::from_static(b"data: caf")).is_empty());
        assert!(decoder.push(&Bytes::from_static(&[0xC3])).is_empty());
        let events = decoder.push(&Bytes::from_static(&[
            0xA9, b'\r', b'\n', b'\r', b'\n', b'd', b'a', b't', b'a', b':', b' ', b'x',
        ]));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "café");
        assert!(decoder.push(&Bytes::new()).is_empty());
        let tail = decoder.finish();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].data, "x");
    }

    #[test]
    fn ignores_null_and_retry_fields() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(&Bytes::from_static(b"id: 1\nretry: 1000\ndata: ok\n\n"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "ok");
    }
}
