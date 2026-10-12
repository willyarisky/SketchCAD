//! The tag-length-value tree that SketchUp's `.dat` streams are built from.
//!
//! Every record is a little-endian `u16` key, a little-endian `u32` byte length, then that many
//! bytes of payload — and a payload is very often just more records. The whole of `model.dat` is
//! one record wrapping the rest, so a file of *n* bytes opens with a length of *n* − 6.
//!
//! The framing is universal, but what each key *means* is not documented anywhere: the numbers
//! are SketchUp's own enum. Options and attribute dictionaries name themselves (a key introduces
//! the literal string "LengthUnit"), while geometry does not. So this module gives you the shape
//! of a file and leaves naming to whoever is reading the dump.

/// Bytes of framing before each payload: `u16` key + `u32` length.
pub const HEADER: usize = 6;

#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    pub key: u16,
    pub data: &'a [u8],
}

impl<'a> Record<'a> {
    /// The payload read as more records, if it parses cleanly as a whole.
    ///
    /// Binary leaves can parse by coincidence, so this is a good guess rather than a fact; for
    /// anything load-bearing, check the key.
    pub fn children(&self) -> Option<Vec<Record<'a>>> {
        if self.data.len() < HEADER {
            return None;
        }
        parse(self.data)
    }

    /// The payload as text, when it is entirely printable.
    pub fn text(&self) -> Option<&'a str> {
        let s = std::str::from_utf8(self.data).ok()?;
        s.chars().all(|c| !c.is_control()).then_some(s)
    }

    pub fn u32(&self) -> Option<u32> {
        Some(u32::from_le_bytes(self.data.try_into().ok()?))
    }

    pub fn u16(&self) -> Option<u16> {
        Some(u16::from_le_bytes(self.data.try_into().ok()?))
    }
}

/// Walk `buf` as a flat sequence of records, stopping at the first malformed one.
pub fn records(buf: &[u8]) -> Reader<'_> {
    Reader { buf, pos: 0 }
}

/// Every record in `buf`, or `None` unless they tile it exactly. Used to tell a nested payload
/// from a binary leaf.
pub fn parse(buf: &[u8]) -> Option<Vec<Record<'_>>> {
    let mut r = records(buf);
    let out: Vec<Record> = r.by_ref().collect();
    (r.pos == buf.len() && !out.is_empty()).then_some(out)
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Reader<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Record<'a>> {
        let head = self.buf.get(self.pos..self.pos + HEADER)?;
        let key = u16::from_le_bytes([head[0], head[1]]);
        let len = u32::from_le_bytes([head[2], head[3], head[4], head[5]]) as usize;
        let start = self.pos + HEADER;
        let data = self.buf.get(start..start.checked_add(len)?)?;
        self.pos = start + len;
        Some(Record { key, data })
    }
}

/// The outermost record of a `.dat` stream, which wraps everything else.
pub fn root(buf: &[u8]) -> Option<Record<'_>> {
    let mut r = records(buf);
    let first = r.next()?;
    // A well-formed stream is exactly one record; anything left over means we misread it.
    (r.pos == buf.len()).then_some(first)
}

/// An indented dump of the tree, for working out what the keys mean.
///
/// `max_depth` and `max_bytes` keep a big model readable: `model.dat` from a real file is over a
/// hundred thousand records.
pub fn dump(rec: &Record, max_depth: usize, max_bytes: usize, out: &mut String) {
    fn go(r: &Record, depth: usize, max_depth: usize, max_bytes: usize, out: &mut String) {
        use std::fmt::Write;
        let pad = "  ".repeat(depth);
        let _ = write!(out, "{pad}key 0x{:04x} len {}", r.key, r.data.len());
        if let Some(t) = r.text().filter(|t| !t.is_empty()) {
            let _ = writeln!(out, " text {t:?}");
            return;
        }
        match r.children().filter(|_| depth < max_depth) {
            Some(kids) => {
                let _ = writeln!(out, " ({} children)", kids.len());
                for k in kids {
                    go(&k, depth + 1, max_depth, max_bytes, out);
                }
            }
            None => {
                let n = r.data.len().min(max_bytes);
                let hex: String = r.data[..n].iter().map(|b| format!("{b:02x}")).collect();
                let _ = writeln!(out, " raw {hex}{}", if n < r.data.len() { "..." } else { "" });
            }
        }
    }
    go(rec, 0, max_depth, max_bytes, out);
}

/// How often each key appears, deepest-first. The starting point for naming keys: a model with
/// one more face than another differs by a predictable handful of these.
pub fn key_census(rec: &Record) -> std::collections::BTreeMap<u16, usize> {
    let mut counts = std::collections::BTreeMap::new();
    fn go(r: &Record, counts: &mut std::collections::BTreeMap<u16, usize>, depth: usize) {
        *counts.entry(r.key).or_insert(0) += 1;
        if depth > 64 {
            return;
        }
        if let Some(kids) = r.children() {
            for k in kids {
                go(&k, counts, depth + 1);
            }
        }
    }
    go(rec, &mut counts, 0);
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(key: u16, payload: &[u8]) -> Vec<u8> {
        let mut v = key.to_le_bytes().to_vec();
        v.extend((payload.len() as u32).to_le_bytes());
        v.extend(payload);
        v
    }

    #[test]
    fn reads_a_nested_tree() {
        let inner = [rec(0x36b4, b"ModelProperties"), rec(0x36b0, &[])].concat();
        let file = rec(0x01f4, &inner);
        let root = root(&file).expect("one wrapping record");
        assert_eq!(root.key, 0x01f4);
        assert_eq!(root.data.len(), file.len() - HEADER, "length covers everything after the header");
        let kids = root.children().expect("payload is records");
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].text(), Some("ModelProperties"));
        assert_eq!(kids[1].data.len(), 0);
    }

    /// A length running past the end is rejected rather than panicking or reading wild memory.
    #[test]
    fn truncated_records_stop_the_walk() {
        let mut file = rec(0x0064, b"abcdef");
        file.truncate(file.len() - 2);
        assert!(root(&file).is_none());
        assert_eq!(records(&file).count(), 0);
        // A plausible header promising more than exists must not parse either.
        let bogus = [0x64u8, 0x00, 0xff, 0xff, 0xff, 0x7f];
        assert_eq!(records(&bogus).count(), 0);
    }

    #[test]
    fn trailing_bytes_are_not_a_tree() {
        let mut file = rec(0x0064, b"ab");
        file.push(0x99);
        assert!(root(&file).is_none(), "leftovers mean we misread the framing");
    }
}
