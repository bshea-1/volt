use std::collections::HashMap;

pub const SCRATCH_OFFSET: u32 = 64;
pub const DATA_START: u32 = 1024;

#[derive(Debug, Clone)]
pub struct StringEntry {
    pub offset: u32,
    pub len: u32,
}

pub struct StringTable {
    strings: HashMap<String, StringEntry>,
    data: Vec<u8>,
    current_offset: u32,
}

impl StringTable {
    pub fn new() -> Self {
        Self {
            strings: HashMap::new(),
            data: Vec::new(),
            current_offset: DATA_START,
        }
    }

    pub fn insert(&mut self, s: &str) -> StringEntry {
        if let Some(entry) = self.strings.get(s) {
            return entry.clone();
        }

        let bytes = s.as_bytes();
        let entry = StringEntry {
            offset: self.current_offset,
            len: bytes.len() as u32,
        };

        self.strings.insert(s.to_string(), entry.clone());
        self.data.extend_from_slice(bytes);
        self.current_offset += bytes.len() as u32;

        entry
    }

    pub fn get(&self, s: &str) -> Option<StringEntry> {
        self.strings.get(s).cloned()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }
}
