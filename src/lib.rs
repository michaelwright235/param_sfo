#![doc = include_str!("../README.md")]

use binrw::{BinRead, BinWrite};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// The most common version of PARAM.SFO \(1.1\).
pub const VERSION_1_1: [u8; 4] = [1, 1, 0, 0];
const U32_SIZE: u32 = size_of::<u32>() as u32;

/// Any possible error that may happen during working with a
/// PARAM.SFO.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),
    #[error("Invalid format: {0}")]
    InvalidFormat(#[from] binrw::Error),
    #[error("Invalid UTF8 string: {0}")]
    FromUtf8Error(#[from] std::string::FromUtf8Error),
    #[error("Exceeded the maximum data length of the current entry.")]
    MaxLenExceeded,
    #[error("The size of keys, values and max_len vecs doensn't match.")]
    InvalidParts,
    #[error("The key contains a null byte or exceeds the limit of u32::MAX.")]
    InvalidKey,
}

#[derive(BinRead, BinWrite, Debug)]
#[brw(big, magic = b"\0PSF")]
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
    /// An array of bytes
    Bytes = 0x4,
    /// A UTF8 string with a \0 termitanion byte
    Utf8 = 0x204,
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

/// The value of the data.
///
/// There are 3 variant of a value.
/// `Bytes` (a.k.a "utf8 Special Mode", "utf-s"), `Utf8` string and `Int` (u32 integer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// An array of bytes, also known as "utf8 Special Mode" or "utf-s".
    /// In most cases these are not actual strings though.
    Bytes(Vec<u8>),
    /// A Utf-8 string.
    Utf8(String),
    /// A u32 integer.
    Int(u32),
}

impl Value {
    pub(crate) fn data_fmt(&self) -> DataFmt {
        match self {
            Value::Bytes(_) => DataFmt::Bytes,
            Value::Utf8(_) => DataFmt::Utf8,
            Value::Int(_) => DataFmt::Int,
        }
    }

    pub(crate) fn len(&self) -> u32 {
        match self {
            Value::Bytes(v) => v.len() as u32,
            Value::Utf8(v) => {
                if !v.is_empty() {
                    v.len() as u32 + 1 // + \0
                } else {
                    0
                }
            }
            Value::Int(_) => U32_SIZE,
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Utf8(value.into())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Utf8(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Int(value)
    }
}

/// The data of an entry.
///
/// It contains a [Value] and the maximum length of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    value: Value,
    max_len: u32,
}

impl Data {
    /// Creates a new [Data] with the `value` and the `max_len`
    /// \(maximum length of it\).
    ///
    /// If the value is an integer, `max_len` is ignored (u32 is always 4 bytes).
    ///
    /// Returns an [Error] if the `value`'s length is greater then the `max_len` or [u32::MAX].
    pub fn new(value: Value, mut max_len: u32) -> Result<Self, Error> {
        if let Value::Int(_) = &value {
            max_len = U32_SIZE;
        }
        if value.len() > max_len {
            Err(Error::MaxLenExceeded)
        } else {
            Ok(Self { value, max_len })
        }
    }

    /// Returns an immutable reference to the underlying Value.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Sets and replaces the value with the given one.
    ///
    /// If the new value is an integer, `max_len` is set to 4 (u32 is always 4 bytes).
    ///
    /// Returns an [Error] if its length is greater then the `max_len`.
    pub fn set_value(&mut self, value: Value) -> Result<(), Error> {
        if let Value::Int(_) = &value {
            self.max_len = U32_SIZE;
        }
        if value.len() > self.max_len {
            Err(Error::MaxLenExceeded)
        } else {
            self.value = value;
            Ok(())
        }
    }

    /// Returns the maximum length of the data.
    pub fn max_len(&self) -> u32 {
        self.max_len
    }

    /// Sets the maximum length of the data.
    ///
    /// If the value is an integer, this method does nothing and returns [Ok]
    /// (u32 is always 4 bytes).
    ///
    /// Returns an [Error] if the `max_len` is greater then the value's length.
    pub fn set_max_len(&mut self, max_len: u32) -> Result<(), Error> {
        if let Value::Int(_) = &self.value {
            return Ok(());
        }
        if self.value.len() > max_len {
            Err(Error::MaxLenExceeded)
        } else {
            self.max_len = max_len;
            Ok(())
        }
    }
}

/// The key of an entry.
///
/// A key itself is a string that contains only uppercase symbols.
/// When using [Key::new()] the given string is automatically converted
/// to the uppercase equivalent of itself. This is a requirement of the format.
///
/// This struct dereferences to &str, thus all its methods are available.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(String);

impl Key {
    /// Creates a new [Key] from a given string reference.
    ///
    /// Returns an error, if it contains a `\0` byte or its length
    /// is greater than `u32::MAX`
    pub fn new<T: AsRef<str>>(key: T) -> Result<Self, Error> {
        if key.as_ref().len() > u32::MAX as usize || key.as_ref().contains('\0') {
            return Err(Error::InvalidKey);
        }
        let key = key.as_ref().to_uppercase();
        Ok(Self(key))
    }

    /// Returns a string slice representing the key.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes itself and returns the underlying [String].
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

impl TryFrom<&str> for Key {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for Key {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSFO {
    version: [u8; 4],
    // BTreeMap is used to automatically sort the keys
    // by alphabetical order, as the format requires it
    entries: BTreeMap<Key, Data>,
}

impl ParamSFO {
    /// Creates an empty PARAM.SFO.
    ///
    /// By default, the version of PARAM.SFO is set to 1.1.
    /// Use [set_version](Self::set_version) to change it.
    pub fn new() -> Self {
        Self {
            version: VERSION_1_1,
            entries: BTreeMap::new(),
        }
    }

    /// Creates a new PARAM.SFO with the given entries.
    ///
    /// By default, the version of PARAM.SFO is set to 1.1.
    /// Use [set_version](Self::set_version) to change it.
    pub fn with_entries(entries: BTreeMap<Key, Data>) -> Self {
        Self {
            version: VERSION_1_1,
            entries,
        }
    }

    /// Reads a PARAM.SFO from a given stream.
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
            let key = Key::new(Self::read_null_terminated_string(&mut stream)?)?;

            stream.seek(SeekFrom::Start(
                (entry.data_offset + header.data_table_start) as u64,
            ))?;
            let mut data_vec: Vec<u8> = vec![0; entry.data_len as usize];
            stream.read_exact(&mut data_vec)?;
            let data = match entry.data_fmt {
                DataFmt::Utf8 => Value::Utf8({
                    data_vec.pop(); // remove \0
                    String::from_utf8(data_vec)?
                }),
                DataFmt::Bytes => Value::Bytes(data_vec),
                DataFmt::Int => Value::Int({
                    if entry.data_len != 0 {
                        let (int_bytes, _) = data_vec.split_at(U32_SIZE as usize);
                        let res: [u8; U32_SIZE as usize] =
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
                Data {
                    value: data,
                    max_len: entry.data_max_len,
                },
            );
        }

        Ok(Self { version, entries })
    }

    /// Reads a PARAM.SFO from a slice of bytes.
    pub fn from_bytes<T: AsRef<[u8]>>(bytes: T) -> Result<Self, Error> {
        let cursor = Cursor::new(bytes);
        Self::from_reader(cursor)
    }

    /// Reads a PARAM.SFO from a file located at `path`.
    pub fn from_file<T: AsRef<Path>>(path: T) -> Result<Self, Error> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(file)
    }

    /// Creates a PARAM.SFO from its parts (vectors of keys, values, max lengths).
    ///
    /// Returns an [Error] if the length of given vectors isn't the same.
    pub fn from_parts(
        keys: Vec<String>,
        values: Vec<Value>,
        max_lens: Vec<u32>,
    ) -> Result<ParamSFO, Error> {
        use std::iter::zip;
        if keys.len() != values.len() || values.len() != max_lens.len() {
            return Err(Error::InvalidParts);
        }
        let mut entries = BTreeMap::new();
        for ((k, v), l) in zip(zip(keys, values), max_lens) {
            entries.insert(Key::new(k)?, Data::new(v, l)?);
        }
        Ok(ParamSFO::with_entries(entries))
    }

    /// Writes the PARAM.SFO to the given stream.
    pub fn to_writer<T: Write>(&self, mut stream: T) -> Result<(), Error> {
        let mut key_table = Vec::with_capacity(1024);
        let mut data_table = Vec::with_capacity(1024);
        let mut index_table_entries = Cursor::new(Vec::with_capacity(1024));

        for (key, entry) in &self.entries {
            let current_key_offset = key_table.len();
            let current_data_offset = data_table.len();

            key_table.extend(key.as_bytes());
            key_table.push(0);

            match &entry.value {
                Value::Bytes(v) => data_table.extend(v),
                Value::Utf8(v) => {
                    data_table.extend(v.as_bytes());
                    data_table.push(0);
                }
                Value::Int(v) => data_table.extend(v.to_le_bytes()),
            };

            let data_zero_bytes = (entry.max_len - entry.value.len()) as usize;
            data_table.extend(&vec![0; data_zero_bytes]);

            let table_entry = IndexTableEntry {
                key_offset: current_key_offset as u16,
                data_fmt: entry.value.data_fmt(),
                data_len: entry.value.len() as u32,
                data_max_len: entry.max_len,
                data_offset: current_data_offset as u32,
            };

            table_entry.write(&mut index_table_entries)?;
        }

        let index_table_entries = index_table_entries.into_inner();
        let key_padding = 4 - ((0x14 + index_table_entries.len() + key_table.len()) % 4);

        let header = Header {
            version: self.version,
            key_table_start: 0x14 + index_table_entries.len() as u32,
            data_table_start: 0x14
                + (index_table_entries.len() + key_table.len() + key_padding) as u32,
            tables_entries: self.entries.len() as u32,
        };
        let mut header_bytes = Cursor::new(Vec::with_capacity(0x14));
        header.write(&mut header_bytes)?;
        let header_bytes = header_bytes.into_inner();

        stream.write_all(&header_bytes)?;
        stream.write_all(&index_table_entries)?;
        stream.write_all(&key_table)?;
        stream.write_all(&vec![0; key_padding])?;
        stream.write_all(&data_table)?;

        Ok(())
    }

    /// Writes the PARAM.SFO to a vector of bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut cursor = Cursor::new(Vec::with_capacity(1024));
        self.to_writer(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    /// Writes the PARAM.SFO to a file located at `path`.
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let file = std::fs::File::create(path)?;
        self.to_writer(file)
    }

    /// Returns an immutable reference to the entries of the current PARAM.SFO.
    pub fn entries(&self) -> &BTreeMap<Key, Data> {
        &self.entries
    }

    /// Returns a mutable reference to the entries of the current PARAM.SFO.
    pub fn entries_mut(&mut self) -> &mut BTreeMap<Key, Data> {
        &mut self.entries
    }

    /// Sets and replaces the current entries with the given ones.
    pub fn set_entries(&mut self, entries: BTreeMap<Key, Data>) {
        self.entries = entries;
    }

    /// Returns the version of PARAM.SFO.
    pub fn version(&self) -> [u8; 4] {
        self.version
    }

    /// Sets the version of PARAM.SFO.
    pub fn set_version(&mut self, version: [u8; 4]) {
        self.version = version
    }

    fn read_null_terminated_string<T: Read>(mut stream: T) -> Result<String, Error> {
        let mut key_vec = vec![];
        loop {
            let mut byte = [0u8];
            stream.read_exact(&mut byte)?;
            if byte[0] == 0 {
                break;
            }
            key_vec.push(byte[0]);
        }
        let key_string = String::from_utf8(key_vec)?;
        Ok(key_string)
    }
}

impl Default for ParamSFO {
    fn default() -> Self {
        Self::new()
    }
}

/// A macro that helps building a PARAM.SFO from scratch.
///
/// The format of each line is `key => [value, max_len]`.
/// For the `key` and `value` you may use either a value
/// itself or an expression/variable. If a value is an integer,
/// `max_len` is set to 4 anyway.
///
/// ## Example
///
/// ```rust
/// use sfo::{ParamSFO, Error, param_sfo};
///
/// let key = "MYKEY";
/// let value = "Hello world!";
///
/// let sfo: Result<ParamSFO, Error> = param_sfo! {
///     "STRINGKEY" => ["Value", 4],
///     "BYTESKEY" => [vec![1,2,3], 8],
///     "INTKEY" => [123, 4],
///     key => [value, 20]
/// };
/// ```
#[macro_export]
macro_rules! param_sfo {
    ($($key:expr => [$s:expr, $len:expr]),*) => {
        {
            let keys = vec![$($key.to_string()),*];
            let values = vec![$($s.into()),*];
            let max_lens = vec![$($len),*];
            $crate::ParamSFO::from_parts(keys, values, max_lens)
        }
    };
}
