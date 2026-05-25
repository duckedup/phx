pub struct SseParser {
    buffer: Vec<u8>,
    delimiter: SseDelimiter,
}

pub enum SseDelimiter {
    SingleNewline,
    DoubleNewline,
}

pub enum SseLine {
    Data(serde_json::Value),
    Done,
    Raw(String),
}

impl SseParser {
    pub fn new(delimiter: SseDelimiter) -> Self {
        Self {
            buffer: Vec::new(),
            delimiter,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    pub fn next_line(&mut self) -> Option<SseLine> {
        match self.delimiter {
            SseDelimiter::SingleNewline => self.next_single(),
            SseDelimiter::DoubleNewline => self.next_double(),
        }
    }

    fn next_single(&mut self) -> Option<SseLine> {
        let pos = self.buffer.iter().position(|&b| b == b'\n')?;
        let line_bytes = self.buffer[..pos].to_vec();
        self.buffer = self.buffer[pos + 1..].to_vec();

        let line = String::from_utf8_lossy(&line_bytes).trim().to_string();

        if line.is_empty() {
            return Some(SseLine::Raw(String::new()));
        }

        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                return Some(SseLine::Done);
            }
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                return Some(SseLine::Data(json));
            }
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            return Some(SseLine::Data(json));
        }

        Some(SseLine::Raw(line))
    }

    fn next_double(&mut self) -> Option<SseLine> {
        let pos = self.buffer.windows(2).position(|w| w == b"\n\n")?;
        let block_bytes = self.buffer[..pos].to_vec();
        self.buffer = self.buffer[pos + 2..].to_vec();

        let block = String::from_utf8_lossy(&block_bytes).to_string();

        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    return Some(SseLine::Done);
                }
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                    return Some(SseLine::Data(json));
                }
            }
        }

        Some(SseLine::Raw(block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_newline_parses_data_lines() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        parser.push(b"data: {\"text\":\"hello\"}\n");
        match parser.next_line() {
            Some(SseLine::Data(json)) => {
                assert_eq!(json["text"], "hello");
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn single_newline_parses_done() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        parser.push(b"data: [DONE]\n");
        assert!(matches!(parser.next_line(), Some(SseLine::Done)));
    }

    #[test]
    fn single_newline_parses_raw_json() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        parser.push(b"{\"done\":true}\n");
        match parser.next_line() {
            Some(SseLine::Data(json)) => {
                assert_eq!(json["done"], true);
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn double_newline_parses_blocks() {
        let mut parser = SseParser::new(SseDelimiter::DoubleNewline);
        parser.push(b"event: message\ndata: {\"type\":\"text\"}\n\n");
        match parser.next_line() {
            Some(SseLine::Data(json)) => {
                assert_eq!(json["type"], "text");
            }
            other => panic!("expected Data, got {other:?}"),
        }
    }

    #[test]
    fn partial_data_waits_for_delimiter() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        parser.push(b"data: {\"partial\"");
        assert!(parser.next_line().is_none());
        parser.push(b":true}\n");
        assert!(matches!(parser.next_line(), Some(SseLine::Data(_))));
    }

    #[test]
    fn multiple_lines_in_one_push() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        parser.push(b"data: {\"a\":1}\ndata: {\"b\":2}\n");
        assert!(matches!(parser.next_line(), Some(SseLine::Data(_))));
        assert!(matches!(parser.next_line(), Some(SseLine::Data(_))));
        assert!(parser.next_line().is_none());
    }

    #[test]
    fn empty_lines_returned_as_raw() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        parser.push(b"\n");
        match parser.next_line() {
            Some(SseLine::Raw(s)) => assert!(s.is_empty()),
            other => panic!("expected empty Raw, got {other:?}"),
        }
    }

    #[test]
    fn multibyte_utf8_split_across_chunks() {
        let mut parser = SseParser::new(SseDelimiter::SingleNewline);
        let emoji = "data: {\"text\":\"hello 🌍\"}\n";
        let bytes = emoji.as_bytes();
        // Split in the middle of the 4-byte emoji (🌍 = F0 9F 8C 8D)
        let split = bytes.len() - 3;
        parser.push(&bytes[..split]);
        assert!(parser.next_line().is_none());
        parser.push(&bytes[split..]);
        match parser.next_line() {
            Some(SseLine::Data(json)) => {
                assert_eq!(json["text"], "hello 🌍");
            }
            other => panic!("expected Data with emoji, got {other:?}"),
        }
    }

    impl std::fmt::Debug for SseLine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                SseLine::Data(v) => write!(f, "Data({v})"),
                SseLine::Done => write!(f, "Done"),
                SseLine::Raw(s) => write!(f, "Raw({s:?})"),
            }
        }
    }
}
