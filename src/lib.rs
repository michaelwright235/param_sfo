use binrw::{BinRead, BinWrite};
use std::{
    collections::BTreeMap, io::{Cursor, Read, Seek, SeekFrom, Write}, path::Path,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),
    #[error("Invalid format: {0}")]
    InvalidFormat(#[from] binrw::Error),
    #[error("Invalid UTF8 string: {0}")]
    FromUtf8Error(#[from] std::string::FromUtf8Error),
    #[error("Exceeded the maximum data length of the current entry")]
    MaxLenExceeded
}

#[derive(BinRead, BinWrite, Debug)]
#[brw(big, magic = 0x00505346u32)]
struct Header {
    /// The version of SFO. Usually 1.1
    #[brw(big)]
    version: [u8; 4],

    /// Start offset of key_table
    #[brw(little)]
    key_table_start: u32,

    /// Start offset of data_table
    #[brw(little)]
    data_table_start: u32,

    /// Number of entries in all tables
    #[brw(little)]
    tables_entries: u32,
}

#[derive(BinRead, BinWrite, Debug)]
#[brw(little, repr = u16)]
enum DataFmt {
    /// A UTF8 string without a \0 termitanion byte
    UTF8S = 0x4,
    /// A UTF8 string with a \0 termitanion byte
    UTF8 = 0x204,
    /// A u32 integer
    Int = 0x404,
}

#[derive(BinRead, BinWrite, Debug)]
#[brw(little)]
struct IndexTableEntry {
    /// param_key offset (relative to start offset of key_table)
    key_offset: u16,
    /// param_data data type
    data_fmt: DataFmt,
    /// param_data used bytes
    data_len: u32,
    /// param_data total bytes
    data_max_len: u32,
    /// param_data offset (relative to start offset of data_table)
    data_offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// A UTF8 string without a \0 termitanion byte
    UTF8S(String),
    /// A UTF8 string with a \0 termitanion byte
    UTF8(String),
    /// A u32 integer
    Int(u32),
}

impl Data {
    pub(crate) fn data_fmt(&self) -> DataFmt {
        match self {
            Data::UTF8S(_) => DataFmt::UTF8S,
            Data::UTF8(_) => DataFmt::UTF8,
            Data::Int(_) => DataFmt::Int,
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Data::UTF8S(v) => v.len(),
            Data::UTF8(v) => v.len() + 1, // + \0
            Data::Int(_) => size_of::<u32>(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    data: Data,
    max_len: u32,
}

impl Entry {
    pub fn new(data: Data, max_len: u32) -> Result<Self, Error> {
        if data.len() > max_len as usize {
            Err(Error::MaxLenExceeded)
        } else {
            Ok(Self {data, max_len})
        }
    }

    pub fn data(&self) -> &Data {
        &self.data
    }

    pub fn set_data(&mut self, data: Data) -> Result<(), Error> {
        if data.len() > self.max_len as usize {
            Err(Error::MaxLenExceeded)
        } else {
            self.data = data;
            Ok(())
        }
    }

    pub fn max_len(&self) -> u32 {
        self.max_len
    }

    pub fn set_max_len(&mut self, max_len: u32) {
        self.max_len = max_len
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd)]
pub struct Key(String);

impl Key {
    pub fn new(key: impl AsRef<str>) -> Self {
        let key = key.as_ref().to_uppercase();
        Self(key)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::ops::Deref for Key {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSFO {
    version: [u8; 4],
    // We use BTreeMap to automatically sort the keys
    // by alphabetical order, as the format requires it
    entries: BTreeMap<Key, Entry>,
}

impl ParamSFO {
    pub fn from_reader<T: Read + Seek>(mut stream: T) -> Result<Self, Error> {
        stream.seek(SeekFrom::Start(0))?;
        let header = Header::read(&mut stream)?;
        let version = header.version;

        let mut index_table_entries = Vec::with_capacity(header.tables_entries as _);
        stream.seek(SeekFrom::Start(0x14))?;

        for _ in 0..header.tables_entries {
            index_table_entries.push(IndexTableEntry::read(&mut stream)?);
        }

        let mut entries = BTreeMap::new();
        for entry in index_table_entries {
            stream.seek(SeekFrom::Start(
                (entry.key_offset as u32 + header.key_table_start) as u64,
            ))?;
            let key = Key::new(Self::read_null_terminated_string(&mut stream)?);

            stream.seek(SeekFrom::Start(
                (entry.data_offset + header.data_table_start) as u64,
            ))?;
            let mut data_vec: Vec<u8> = vec![0; entry.data_len as usize];
            stream.read_exact(&mut data_vec)?;
            let data = match entry.data_fmt {
                DataFmt::UTF8 => Data::UTF8({
                    let mut s = String::from_utf8(data_vec)?;
                    s.pop();
                    s
                }),
                DataFmt::UTF8S => Data::UTF8S(String::from_utf8(data_vec)?),
                DataFmt::Int => Data::Int({
                    if entry.data_len != 0 {
                        let (int_bytes, _) = data_vec.split_at(size_of::<u32>());
                        let res: [u8; size_of::<u32>()] =
                            int_bytes.try_into().map_err(|e| binrw::Error::Custom {
                                pos: stream.stream_position().unwrap_or_default(),
                                err: Box::new(e),
                            })?;
                        u32::from_le_bytes(res)
                    } else {
                        0
                    }

                }),
            };
            entries.insert(
                key,
                Entry {
                    data,
                    max_len: entry.data_max_len,
                },
            );
        }

        Ok(Self { version, entries })
    }

    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, Error> {
        let cursor = Cursor::new(bytes);
        Self::from_reader(cursor)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(file)
    }

    pub fn to_writer<T: Write>(&self, mut writer: T) -> Result<(), Error> {
        let mut key_table = Vec::with_capacity(512);
        let mut data_table = Vec::with_capacity(512);
        let mut index_table_entries = Cursor::new(Vec::with_capacity(512));

        for (key, entry) in &self.entries {
            let current_key_offset = key_table.len();
            let current_data_offset = data_table.len();

            key_table.extend(key.as_bytes());
            key_table.push(0);

            match &entry.data {
                Data::UTF8S(v) => data_table.extend(v.as_bytes()),
                Data::UTF8(v) => {
                    data_table.extend(v.as_bytes());
                    data_table.push(0);
                },
                Data::Int(v) => data_table.extend(v.to_le_bytes()),
            };

            let data_zero_bytes = entry.max_len as usize - entry.data.len();
            data_table.extend(&vec![0; data_zero_bytes]);

            let table_entry = IndexTableEntry {
                key_offset: current_key_offset as u16,
                data_fmt: entry.data.data_fmt(),
                data_len: entry.data.len() as u32,
                data_max_len: entry.max_len,
                data_offset: current_data_offset as u32,
            };

            table_entry.write(&mut index_table_entries)?;
        }

        let index_table_entries = index_table_entries.into_inner();
        let key_padding = 4-((0x14 + index_table_entries.len() + key_table.len()) % 4);

        let header = Header {
            version: self.version,
            key_table_start: 0x14 + index_table_entries.len() as u32,
            data_table_start: 0x14 + (index_table_entries.len() + key_table.len() + key_padding) as u32,
            tables_entries: self.entries.len() as u32,
        };
        let mut header_bytes = Cursor::new(Vec::with_capacity(0x14));
        header.write(&mut header_bytes)?;
        let header_bytes = header_bytes.into_inner();

        writer.write_all(&header_bytes)?;
        writer.write_all(&index_table_entries)?;
        writer.write_all(&key_table)?;
        writer.write_all(&vec![0; key_padding])?;
        writer.write_all(&data_table)?;

        Ok(())
    }

    pub fn entries(&self) -> &BTreeMap<Key, Entry> {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut BTreeMap<Key, Entry> {
        &mut self.entries
    }

    pub fn version(&self) -> [u8; 4] {
        self.version
    }

    pub fn set_version(&mut self, version: [u8; 4]) {
        self.version = version
    }

    fn read_null_terminated_string<T: Read>(mut stream: T) -> Result<String, Error> {
        let mut key_vec = vec![];
        loop {
            let mut byte = [0u8];
            stream.read_exact(&mut byte)?;
            key_vec.push(byte[0]);
            if byte[0] == 0 {
                break;
            }
        }
        key_vec.pop();
        let key_string = String::from_utf8(key_vec)?;
        Ok(key_string)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use crate::ParamSFO;

    #[test]
    fn test() {
        let a = ParamSFO::from_file("./tests/PARAM.SFO").unwrap();
        let mut b = File::create("./tests/PARAM2.SFO").unwrap();
        a.to_writer(&mut b).unwrap();
        dbg!(a);
    }
}
