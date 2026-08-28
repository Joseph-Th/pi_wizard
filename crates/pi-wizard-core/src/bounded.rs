use std::collections::VecDeque;

/// Fixed-capacity byte ring used for diagnostics where newest data is most useful.
#[derive(Clone, Debug)]
pub struct ByteRing {
    capacity: usize,
    bytes: VecDeque<u8>,
    dropped_bytes: u64,
}

impl ByteRing {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ByteRing capacity must be non-zero");
        Self {
            capacity,
            bytes: VecDeque::with_capacity(capacity),
            dropped_bytes: 0,
        }
    }

    pub fn push(&mut self, incoming: &[u8]) {
        if incoming.len() >= self.capacity {
            self.dropped_bytes = self
                .dropped_bytes
                .saturating_add(self.bytes.len() as u64)
                .saturating_add((incoming.len() - self.capacity) as u64);
            self.bytes.clear();
            self.bytes
                .extend(incoming[incoming.len() - self.capacity..].iter().copied());
            return;
        }

        let overflow = self
            .bytes
            .len()
            .saturating_add(incoming.len())
            .saturating_sub(self.capacity);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.dropped_bytes = self.dropped_bytes.saturating_add(overflow as u64);
        }
        self.bytes.extend(incoming.iter().copied());
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[must_use]
    pub const fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
    }
}

/// UTF-8 text buffer that retains only the newest bounded suffix.
#[derive(Clone, Debug)]
pub struct BoundedText {
    max_bytes: usize,
    text: String,
    dropped_bytes: u64,
}

impl BoundedText {
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        assert!(max_bytes > 0, "BoundedText max_bytes must be non-zero");
        Self {
            max_bytes,
            text: String::new(),
            dropped_bytes: 0,
        }
    }

    pub fn append(&mut self, value: &str) {
        self.text.push_str(value);
        self.trim_to_limit();
    }

    pub fn replace(&mut self, value: &str) {
        self.text.clear();
        self.text.push_str(value);
        self.trim_to_limit();
    }

    /// Drops up to `requested` oldest bytes while preserving UTF-8 validity.
    ///
    /// The actual number can be slightly larger than requested when the split
    /// would otherwise land in the middle of a multibyte code point.
    pub fn drop_oldest_bytes(&mut self, requested: usize) -> usize {
        if requested == 0 || self.text.is_empty() {
            return 0;
        }

        let mut split = requested.min(self.text.len());
        while split < self.text.len() && !self.text.is_char_boundary(split) {
            split += 1;
        }
        self.text.drain(..split);
        self.dropped_bytes = self.dropped_bytes.saturating_add(split as u64);
        split
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    #[must_use]
    pub const fn dropped_bytes(&self) -> u64 {
        self.dropped_bytes
    }

    fn trim_to_limit(&mut self) {
        if self.text.len() <= self.max_bytes {
            return;
        }

        let requested_drop = self.text.len() - self.max_bytes;
        let mut split = requested_drop;
        while split < self.text.len() && !self.text.is_char_boundary(split) {
            split += 1;
        }
        self.text.drain(..split);
        self.dropped_bytes = self.dropped_bytes.saturating_add(split as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_ring_keeps_newest_bytes_with_fixed_capacity() {
        let mut ring = ByteRing::new(5);
        ring.push(b"abc");
        ring.push(b"defg");

        assert_eq!(ring.to_vec(), b"cdefg");
        assert_eq!(ring.dropped_bytes(), 2);
        assert_eq!(ring.len(), 5);
    }

    #[test]
    fn byte_ring_large_push_never_exceeds_capacity() {
        let mut ring = ByteRing::new(4);
        ring.push(b"ab");
        ring.push(b"0123456789");

        assert_eq!(ring.to_vec(), b"6789");
        assert_eq!(ring.dropped_bytes(), 8);
        assert_eq!(ring.len(), 4);
    }

    #[test]
    fn bounded_text_trims_at_utf8_boundary() {
        let mut text = BoundedText::new(5);
        text.append("ab😀cd");

        assert_eq!(text.as_str(), "cd");
        assert_eq!(text.len_bytes(), 2);
        assert_eq!(text.dropped_bytes(), 6);
    }

    #[test]
    fn bounded_text_replace_treats_partial_tool_output_as_accumulated() {
        let mut text = BoundedText::new(32);
        text.append("old delta");
        text.replace("authoritative accumulated output");

        assert_eq!(text.as_str(), "authoritative accumulated output");
    }

    #[test]
    fn bounded_text_can_drop_an_oldest_prefix_at_utf8_boundary() {
        let mut text = BoundedText::new(32);
        text.append("a😀bc");

        assert_eq!(text.drop_oldest_bytes(2), 5);
        assert_eq!(text.as_str(), "bc");
        assert_eq!(text.dropped_bytes(), 5);
    }
}
