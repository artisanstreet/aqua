use std::collections::HashMap;

use aqua_fonts_system::FaceFingerprint;

const MAGIC: &[u8; 4] = b"AQFI";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    ranges: Vec<(u32, u32)>,
}

impl Coverage {
    pub fn from_codepoints(codepoints: impl IntoIterator<Item = u32>) -> Self {
        let mut sorted: Vec<u32> = codepoints.into_iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for codepoint in sorted {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == codepoint => *end = codepoint,
                _ => ranges.push((codepoint, codepoint)),
            }
        }
        Self { ranges }
    }

    pub fn contains(&self, codepoint: u32) -> bool {
        self.ranges
            .binary_search_by(|(start, end)| {
                if codepoint < *start {
                    std::cmp::Ordering::Greater
                } else if codepoint > *end {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok()
    }

    pub fn range_count(&self) -> usize {
        self.ranges.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    len: u64,
    modified: u64,
    coverage: Coverage,
}

#[derive(Clone, Debug, Default)]
pub struct FallbackIndex {
    records: HashMap<(String, u32), Record>,
    dirty: bool,
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Option<&[u8]> {
        let (head, tail) = self.bytes.split_at_checked(count)?;
        self.bytes = tail;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u64::from_le_bytes)
    }
}

impl FallbackIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn lookup(&self, face: &FaceFingerprint) -> Option<&Coverage> {
        self.records
            .get(&(face.path.clone(), face.index))
            .filter(|record| record.len == face.len && record.modified == face.modified)
            .map(|record| &record.coverage)
    }

    pub fn insert(&mut self, face: &FaceFingerprint, coverage: Coverage) {
        self.records.insert(
            (face.path.clone(), face.index),
            Record {
                len: face.len,
                modified: face.modified,
                coverage,
            },
        );
        self.dirty = true;
    }

    pub fn take_snapshot(&mut self) -> Option<Vec<u8>> {
        self.dirty.then(|| {
            self.dirty = false;
            self.encode()
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut keys: Vec<&(String, u32)> = self.records.keys().collect();
        keys.sort();
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        for key in keys {
            let record = &self.records[key];
            out.extend_from_slice(&(key.0.len() as u32).to_le_bytes());
            out.extend_from_slice(key.0.as_bytes());
            out.extend_from_slice(&key.1.to_le_bytes());
            out.extend_from_slice(&record.len.to_le_bytes());
            out.extend_from_slice(&record.modified.to_le_bytes());
            out.extend_from_slice(&(record.coverage.ranges.len() as u32).to_le_bytes());
            for (start, end) in &record.coverage.ranges {
                out.extend_from_slice(&start.to_le_bytes());
                out.extend_from_slice(&end.to_le_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader { bytes };
        (reader.take(4)? == MAGIC).then_some(())?;
        (reader.u32()? == VERSION).then_some(())?;
        let count = reader.u32()?;
        let mut records = HashMap::new();
        for _ in 0..count {
            let path_len = reader.u32()? as usize;
            let path = String::from_utf8(reader.take(path_len)?.to_vec()).ok()?;
            let index = reader.u32()?;
            let len = reader.u64()?;
            let modified = reader.u64()?;
            let range_count = reader.u32()? as usize;
            let mut ranges = Vec::with_capacity(range_count.min(reader.bytes.len() / 8));
            for _ in 0..range_count {
                let (start, end) = (reader.u32()?, reader.u32()?);
                (start <= end && ranges.last().is_none_or(|(_, last)| *last < start))
                    .then_some(())?;
                ranges.push((start, end));
            }
            records.insert(
                (path, index),
                Record {
                    len,
                    modified,
                    coverage: Coverage { ranges },
                },
            );
        }
        reader.bytes.is_empty().then_some(Self {
            records,
            dirty: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(path: &str, modified: u64) -> FaceFingerprint {
        FaceFingerprint {
            path: path.to_owned(),
            index: 0,
            len: 100,
            modified,
        }
    }

    #[test]
    fn coverage_merges_runs_and_answers_membership() {
        let coverage = Coverage::from_codepoints([5, 3, 4, 4, 10, 20, 21]);
        assert_eq!(coverage.range_count(), 3);
        for hit in [3, 4, 5, 10, 20, 21] {
            assert!(coverage.contains(hit), "{hit}");
        }
        for miss in [0, 2, 6, 9, 11, 19, 22] {
            assert!(!coverage.contains(miss), "{miss}");
        }
    }

    #[test]
    fn encode_decode_round_trips_and_clears_dirty() {
        let mut index = FallbackIndex::new();
        index.insert(&face("a.ttf", 1), Coverage::from_codepoints(0x41..0x5b));
        index.insert(
            &face("b.ttf", 2),
            Coverage::from_codepoints([0x4e00, 0x4e01]),
        );
        let snapshot = index.take_snapshot().expect("dirty");
        assert!(index.take_snapshot().is_none());
        let loaded = FallbackIndex::decode(&snapshot).expect("valid");
        assert!(!loaded.is_dirty());
        assert_eq!(loaded.len(), 2);
        assert!(loaded.lookup(&face("a.ttf", 1)).unwrap().contains(0x42));
        assert!(loaded.lookup(&face("b.ttf", 2)).unwrap().contains(0x4e01));
        assert_eq!(loaded.encode(), snapshot);
    }

    #[test]
    fn a_changed_file_invalidates_its_record() {
        let mut index = FallbackIndex::new();
        index.insert(&face("a.ttf", 1), Coverage::from_codepoints([65]));
        assert!(index.lookup(&face("a.ttf", 2)).is_none());
        index.insert(&face("a.ttf", 2), Coverage::from_codepoints([66]));
        assert_eq!(index.len(), 1);
        assert!(index.lookup(&face("a.ttf", 2)).unwrap().contains(66));
    }

    #[test]
    fn corrupt_or_truncated_input_is_rejected() {
        let mut index = FallbackIndex::new();
        index.insert(&face("a.ttf", 1), Coverage::from_codepoints(0..300));
        let bytes = index.encode();
        assert!(FallbackIndex::decode(&bytes[..bytes.len() - 1]).is_none());
        assert!(FallbackIndex::decode(b"nope").is_none());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(FallbackIndex::decode(&trailing).is_none());
        let mut wrong_version = bytes;
        wrong_version[4] = 9;
        assert!(FallbackIndex::decode(&wrong_version).is_none());
    }
}
