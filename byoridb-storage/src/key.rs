use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

/// Key types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    Vertex = 1,
    Edge = 2,
    System = 3,
    TagIndex = 4,
    EdgeIndex = 5,
}

/// Helper to generate keys for the KV store
/// Key Format:
/// Vertex: PartID (4) + VertexID (8) + KeyType(1) + TagID (4)
/// Edge:   PartID (4) + SrcVertexID (8) + KeyType(1) + EdgeType (4) + Rank (8) + DstVertexID (8)
pub struct KeyUtils;

impl KeyUtils {
    pub fn vertex_key(part_id: u32, vid: i64, tag_id: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(17);
        // In a real distributed system, PartID is calculated from VID.
        // Here we assume it's passed in or 1 for simplified standalone.
        buf.write_u32::<BigEndian>(part_id).unwrap();
        buf.write_i64::<BigEndian>(vid).unwrap();
        buf.push(KeyType::Vertex as u8);
        buf.write_u32::<BigEndian>(tag_id).unwrap();
        buf
    }

    pub fn edge_key(
        part_id: u32,
        src_vid: i64,
        edge_type: u32,
        rank: i64,
        dst_vid: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(33);
        buf.write_u32::<BigEndian>(part_id).unwrap();
        buf.write_i64::<BigEndian>(src_vid).unwrap();
        buf.push(KeyType::Edge as u8);
        buf.write_u32::<BigEndian>(edge_type).unwrap();
        buf.write_i64::<BigEndian>(rank).unwrap();
        buf.write_i64::<BigEndian>(dst_vid).unwrap();
        buf
    }

    pub fn system_key(suffix: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(KeyType::System as u8);
        buf.extend_from_slice(suffix.as_bytes());
        buf
    }

    /// Tag index key
    /// Format: PartID (4) + KeyType::TagIndex (1) + IndexID (4) + PropertyValue(s) + VID (8)
    pub fn tag_index_key(
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
        vid: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);
        buf.write_u32::<BigEndian>(part_id).unwrap();
        buf.push(KeyType::TagIndex as u8);
        buf.write_u32::<BigEndian>(index_id).unwrap();

        // Encode property values
        for value in prop_values {
            value.encode(&mut buf);
        }

        buf.write_i64::<BigEndian>(vid).unwrap();
        buf
    }

    /// Tag index prefix for range scans
    /// Format: PartID (4) + KeyType::TagIndex (1) + IndexID (4)
    pub fn tag_index_prefix(part_id: u32, index_id: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(9);
        buf.write_u32::<BigEndian>(part_id).unwrap();
        buf.push(KeyType::TagIndex as u8);
        buf.write_u32::<BigEndian>(index_id).unwrap();
        buf
    }

    /// Tag index prefix with property values for range scans
    pub fn tag_index_prefix_with_values(
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
    ) -> Vec<u8> {
        let mut buf = Self::tag_index_prefix(part_id, index_id);
        for value in prop_values {
            value.encode(&mut buf);
        }
        buf
    }

    /// Edge index key
    /// Format: PartID (4) + KeyType::EdgeIndex (1) + IndexID (4) + PropertyValue(s) + SrcVID (8) + Rank (8) + DstVID (8)
    pub fn edge_index_key(
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
        src_vid: i64,
        rank: i64,
        dst_vid: i64,
    ) -> Vec<u8> {
        let mut buf = Vec::with_capacity(48);
        buf.write_u32::<BigEndian>(part_id).unwrap();
        buf.push(KeyType::EdgeIndex as u8);
        buf.write_u32::<BigEndian>(index_id).unwrap();

        // Encode property values
        for value in prop_values {
            value.encode(&mut buf);
        }

        buf.write_i64::<BigEndian>(src_vid).unwrap();
        buf.write_i64::<BigEndian>(rank).unwrap();
        buf.write_i64::<BigEndian>(dst_vid).unwrap();
        buf
    }

    /// Edge index prefix for range scans
    pub fn edge_index_prefix(part_id: u32, index_id: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(9);
        buf.write_u32::<BigEndian>(part_id).unwrap();
        buf.push(KeyType::EdgeIndex as u8);
        buf.write_u32::<BigEndian>(index_id).unwrap();
        buf
    }

    /// Edge index prefix with property values for range scans
    pub fn edge_index_prefix_with_values(
        part_id: u32,
        index_id: u32,
        prop_values: &[IndexValue],
    ) -> Vec<u8> {
        let mut buf = Self::edge_index_prefix(part_id, index_id);
        for value in prop_values {
            value.encode(&mut buf);
        }
        buf
    }

    /// Parse VID from tag index key
    /// The VID is always the last 8 bytes of the key
    pub fn parse_tag_index_vid(key: &[u8], _prop_value_len: usize) -> Option<i64> {
        // PartID(4) + KeyType(1) + IndexID(4) + prop_values + VID(8)
        // VID is always at the end
        if key.len() < 17 {
            // Minimum: 9 (header) + 8 (VID)
            return None;
        }
        let vid_offset = key.len() - 8;
        let mut cursor = Cursor::new(&key[vid_offset..]);
        cursor.read_i64::<BigEndian>().ok()
    }

    /// Parse edge from edge index key
    /// The edge info (SrcVID + Rank + DstVID = 24 bytes) is always at the end
    pub fn parse_edge_index_edge(key: &[u8], _prop_value_len: usize) -> Option<(i64, i64, i64)> {
        // PartID(4) + KeyType(1) + IndexID(4) + prop_values + SrcVID(8) + Rank(8) + DstVID(8)
        // Edge info is always at the end (24 bytes)
        if key.len() < 33 {
            // Minimum: 9 (header) + 24 (edge info)
            return None;
        }
        let edge_offset = key.len() - 24;
        let mut cursor = Cursor::new(&key[edge_offset..]);
        let src_vid = cursor.read_i64::<BigEndian>().ok()?;
        let rank = cursor.read_i64::<BigEndian>().ok()?;
        let dst_vid = cursor.read_i64::<BigEndian>().ok()?;
        Some((src_vid, rank, dst_vid))
    }
}

/// Index value for encoding in index keys
/// Values are encoded in a way that preserves sort order
#[derive(Debug, Clone, PartialEq)]
pub enum IndexValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl IndexValue {
    /// Encode the value for index key
    /// Format preserves sort order for range scans
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            IndexValue::Null => {
                buf.push(0x00); // Type marker for NULL
            }
            IndexValue::Bool(b) => {
                buf.push(0x01); // Type marker for bool
                buf.push(if *b { 1 } else { 0 });
            }
            IndexValue::Int(i) => {
                buf.push(0x02); // Type marker for int
                                // XOR with sign bit to preserve sort order
                let encoded = (*i as u64) ^ (1u64 << 63);
                buf.write_u64::<BigEndian>(encoded).unwrap();
            }
            IndexValue::Float(f) => {
                buf.push(0x03); // Type marker for float
                                // IEEE 754 encoding that preserves sort order
                let bits = f.to_bits();
                let encoded = if bits & (1u64 << 63) != 0 {
                    !bits // Negative: flip all bits
                } else {
                    bits ^ (1u64 << 63) // Positive: flip sign bit
                };
                buf.write_u64::<BigEndian>(encoded).unwrap();
            }
            IndexValue::String(s) => {
                buf.push(0x04); // Type marker for string
                                // Length-prefixed string (max 255 bytes for index)
                let bytes = s.as_bytes();
                let len = std::cmp::min(bytes.len(), 255);
                buf.push(len as u8);
                buf.extend_from_slice(&bytes[..len]);
            }
        }
    }

    /// Get the encoded length of this value
    pub fn encoded_len(&self) -> usize {
        match self {
            IndexValue::Null => 1,
            IndexValue::Bool(_) => 2,
            IndexValue::Int(_) => 9,
            IndexValue::Float(_) => 9,
            IndexValue::String(s) => 2 + std::cmp::min(s.len(), 255),
        }
    }

    /// Decode a value from bytes
    pub fn decode(data: &[u8]) -> Option<(Self, usize)> {
        if data.is_empty() {
            return None;
        }

        match data[0] {
            0x00 => Some((IndexValue::Null, 1)),
            0x01 if data.len() >= 2 => Some((IndexValue::Bool(data[1] != 0), 2)),
            0x02 if data.len() >= 9 => {
                let mut cursor = Cursor::new(&data[1..9]);
                let encoded = cursor.read_u64::<BigEndian>().ok()?;
                let value = (encoded ^ (1u64 << 63)) as i64;
                Some((IndexValue::Int(value), 9))
            }
            0x03 if data.len() >= 9 => {
                let mut cursor = Cursor::new(&data[1..9]);
                let encoded = cursor.read_u64::<BigEndian>().ok()?;
                let bits = if encoded & (1u64 << 63) != 0 {
                    encoded ^ (1u64 << 63)
                } else {
                    !encoded
                };
                let value = f64::from_bits(bits);
                Some((IndexValue::Float(value), 9))
            }
            0x04 if data.len() >= 2 => {
                let len = data[1] as usize;
                if data.len() >= 2 + len {
                    let s = String::from_utf8_lossy(&data[2..2 + len]).to_string();
                    Some((IndexValue::String(s), 2 + len))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_value_int_sort_order() {
        let values = [
            IndexValue::Int(-100),
            IndexValue::Int(-1),
            IndexValue::Int(0),
            IndexValue::Int(1),
            IndexValue::Int(100),
        ];

        let mut encoded: Vec<Vec<u8>> = values
            .iter()
            .map(|v| {
                let mut buf = Vec::new();
                v.encode(&mut buf);
                buf
            })
            .collect();

        let original = encoded.clone();
        encoded.sort();
        assert_eq!(encoded, original, "Int values should maintain sort order");
    }

    #[test]
    fn test_index_value_string_sort_order() {
        let values = [
            IndexValue::String("aaa".to_string()),
            IndexValue::String("aab".to_string()),
            IndexValue::String("bbb".to_string()),
        ];

        let mut encoded: Vec<Vec<u8>> = values
            .iter()
            .map(|v| {
                let mut buf = Vec::new();
                v.encode(&mut buf);
                buf
            })
            .collect();

        let original = encoded.clone();
        encoded.sort();
        assert_eq!(
            encoded, original,
            "String values should maintain sort order"
        );
    }

    #[test]
    fn test_index_value_roundtrip() {
        let values = vec![
            IndexValue::Null,
            IndexValue::Bool(true),
            IndexValue::Bool(false),
            IndexValue::Int(-12345),
            IndexValue::Int(0),
            IndexValue::Int(67890),
            IndexValue::Float(-2.5),
            IndexValue::Float(0.0),
            IndexValue::Float(1.25),
            IndexValue::String("hello".to_string()),
            IndexValue::String("".to_string()),
        ];

        for value in values {
            let mut buf = Vec::new();
            value.encode(&mut buf);
            let (decoded, len) = IndexValue::decode(&buf).expect("decode failed");
            assert_eq!(decoded, value);
            assert_eq!(len, buf.len());
        }
    }

    #[test]
    fn test_tag_index_key() {
        let key = KeyUtils::tag_index_key(
            1,
            100,
            &[IndexValue::String("Alice".to_string()), IndexValue::Int(30)],
            12345,
        );

        // Verify prefix
        let prefix = KeyUtils::tag_index_prefix(1, 100);
        assert!(key.starts_with(&prefix));

        // Parse VID
        let prop_len = IndexValue::String("Alice".to_string()).encoded_len()
            + IndexValue::Int(30).encoded_len();
        let vid = KeyUtils::parse_tag_index_vid(&key, prop_len);
        assert_eq!(vid, Some(12345));
    }

    #[test]
    fn test_edge_index_key() {
        let key = KeyUtils::edge_index_key(1, 200, &[IndexValue::Int(100)], 1001, 0, 1002);

        // Verify prefix
        let prefix = KeyUtils::edge_index_prefix(1, 200);
        assert!(key.starts_with(&prefix));

        // Parse edge
        let prop_len = IndexValue::Int(100).encoded_len();
        let edge = KeyUtils::parse_edge_index_edge(&key, prop_len);
        assert_eq!(edge, Some((1001, 0, 1002)));
    }
}
