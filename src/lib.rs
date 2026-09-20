#![doc = include_str!("../README.MD")]

use binrw::{BinRead, BinWrite};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// The most common `PARAM.SFO` version (1.1).
pub const VERSION_1_1: [u8; 4] = [1, 1, 0, 0];
const U32_SIZE: u32 = size_of::<u32>() as u32;

/// An error that can occur when working with a `PARAM.SFO` file.
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
    /// The SFO version, usually 1.1.
    #[brw(big)]
    version: [u8; 4],

    /// The offset of the key table from the start of the file.
    #[brw(little)]
    key_table_start: u32,

    /// The offset of the data table from the start of the file.
    #[brw(little)]
    data_table_start: u32,

    /// The number of entries in each table.
    #[brw(little)]
    tables_entries: u32,
}

#[derive(BinRead, BinWrite, Debug)]
#[brw(little, repr = u16)]
enum DataFmt {
    /// A byte array.
    Bytes = 0x4,
    /// A null-terminated UTF-8 string.
    Utf8 = 0x204,
    /// An unsigned 32-bit integer.
    Int = 0x404,
}

#[derive(BinRead, BinWrite, Debug)]
#[brw(little)]
struct IndexTableEntry {
    /// The key offset relative to the start of the key table.
    key_offset: u16,
    /// The data type of the value.
    data_fmt: DataFmt,
    /// The number of bytes used by the value.
    data_len: u32,
    /// The number of bytes allocated for the value.
    data_max_len: u32,
    /// The value offset relative to the start of the data table.
    data_offset: u32,
}

/// An entry value: a byte array, a UTF-8 string, or an unsigned 32-bit integer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A byte array, also known as "UTF-8 Special Mode" or "utf-s".
    /// The bytes do not necessarily represent a string.
    Bytes(Vec<u8>),
    /// A UTF-8 string.
    Utf8(String),
    /// An unsigned 32-bit integer.
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

/// An entry's [`Value`] and its maximum encoded length in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    value: Value,
    max_len: u32,
}

impl Data {
    /// Creates an entry with the given value and maximum encoded length in bytes.
    ///
    /// For integer values, `max_len` is set to 4, the size of a `u32`.
    ///
    /// Returns [`Error::MaxLenExceeded`] if the encoded value exceeds `max_len`.
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

    /// Returns a reference to the underlying [`Value`].
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// Replaces the current value with the given value.
    ///
    /// If the new value is an integer, `max_len` is set to 4, the size of a `u32`.
    ///
    /// Returns [`Error::MaxLenExceeded`] if the encoded value exceeds `max_len`.
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

    /// Returns the maximum encoded length in bytes.
    pub fn max_len(&self) -> u32 {
        self.max_len
    }

    /// Sets the maximum encoded length in bytes.
    ///
    /// For integer values, this method leaves the length at 4 and returns `Ok(())`.
    ///
    /// Returns [`Error::MaxLenExceeded`] if `max_len` is less than the encoded value's length.
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
/// Keys are stored as uppercase strings, as required by the format.
/// [`Key::new`] automatically converts the input to uppercase.
///
/// This struct dereferences to [`str`], making its methods available.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key(String);

impl Key {
    /// Creates a [`Key`] by converting the given string to uppercase.
    ///
    /// Returns [`Error::InvalidKey`] if the input contains a null byte or its
    /// length in bytes exceeds [`u32::MAX`].
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

    /// Consumes the key and returns the underlying [`String`].
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

/// An in-memory representation of a `PARAM.SFO` file.
///
/// Stores the file version and a map of [`Key`] to [`Data`]. Entries are kept
/// in alphabetical order by key for serialization. Each entry contains a
/// [`Value`] and its maximum encoded length in bytes.
///
/// Use [`Self::new`], [`Self::with_entries`], or [`param_sfo!`] to create a file.
/// Read an existing file with [`Self::from_file`], [`Self::from_bytes`], or
/// [`Self::from_reader`]. Access or modify its entries with [`Self::entries`]
/// and [`Self::entries_mut`], then serialize it with [`Self::to_file`],
/// [`Self::to_bytes`], or [`Self::to_writer`].
///
/// New files default to version 1.1 ([`VERSION_1_1`]); reading a file preserves
/// the version from its header. Use [`Self::set_version`] to change it.
///
/// # Examples
///
/// ```
/// use sfo::{Data, Key, ParamSFO};
///
/// let mut sfo = ParamSFO::new();
/// sfo.entries_mut().insert(
///     Key::new("TITLE")?,
///     Data::new("My game".into(), 128)?,
/// );
///
/// let bytes = sfo.to_bytes()?;
/// let restored = ParamSFO::from_bytes(&bytes)?;
/// assert_eq!(restored, sfo);
/// # Ok::<(), sfo::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSFO {
    version: [u8; 4],
    // BTreeMap keeps the keys in alphabetical order, as required by the format.
    entries: BTreeMap<Key, Data>,
}

impl ParamSFO {
    /// Creates an empty `PARAM.SFO`.
    ///
    /// The version defaults to 1.1.
    /// Use [set_version](Self::set_version) to change it.
    pub fn new() -> Self {
        Self {
            version: VERSION_1_1,
            entries: BTreeMap::new(),
        }
    }

    /// Creates a `PARAM.SFO` with the given entries.
    ///
    /// The version defaults to 1.1.
    /// Use [set_version](Self::set_version) to change it.
    pub fn with_entries(entries: BTreeMap<Key, Data>) -> Self {
        Self {
            version: VERSION_1_1,
            entries,
        }
    }

    /// Reads a `PARAM.SFO` from a stream, seeking to its beginning first.
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

    /// Reads a `PARAM.SFO` from bytes.
    pub fn from_bytes<T: AsRef<[u8]>>(bytes: T) -> Result<Self, Error> {
        let cursor = Cursor::new(bytes);
        Self::from_reader(cursor)
    }

    /// Reads a `PARAM.SFO` from the file at `path`.
    pub fn from_file<T: AsRef<Path>>(path: T) -> Result<Self, Error> {
        let file = std::fs::File::open(path)?;
        Self::from_reader(file)
    }

    /// Creates a `PARAM.SFO` from vectors of keys, values, and maximum lengths.
    ///
    /// The version defaults to 1.1. Keys are converted to uppercase. If multiple
    /// keys become identical, the last entry replaces the earlier ones.
    ///
    /// Returns [`Error::InvalidParts`] if the vectors have different lengths.
    /// Also returns errors from [`Key::new`] and [`Data::new`].
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

    /// Writes the `PARAM.SFO` to the stream at its current position.
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

    /// Serializes the `PARAM.SFO` into a new byte vector.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let mut cursor = Cursor::new(Vec::with_capacity(1024));
        self.to_writer(&mut cursor)?;
        Ok(cursor.into_inner())
    }

    /// Writes the `PARAM.SFO` to the file at `path`.
    ///
    /// Creates the file if it does not exist, or truncates it if it does.
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let file = std::fs::File::create(path)?;
        self.to_writer(file)
    }

    /// Returns a reference to the entries, ordered alphabetically by key.
    pub fn entries(&self) -> &BTreeMap<Key, Data> {
        &self.entries
    }

    /// Returns a mutable reference to the entries for modification.
    pub fn entries_mut(&mut self) -> &mut BTreeMap<Key, Data> {
        &mut self.entries
    }

    /// Replaces all entries with the given map.
    pub fn set_entries(&mut self, entries: BTreeMap<Key, Data>) {
        self.entries = entries;
    }

    /// Returns the four version bytes stored in the `PARAM.SFO` header.
    pub fn version(&self) -> [u8; 4] {
        self.version
    }

    /// Sets the four version bytes stored in the `PARAM.SFO` header.
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

/// Creates a `PARAM.SFO` from key, value, and maximum-length expressions.
///
/// Each entry has the form `key => [value, max_len]`, with entries separated by commas.
/// Keys, values, and maximum lengths can be literals, variables, or expressions.
/// For integer values, `max_len` is always set to 4.
///
/// Returns a `Result<ParamSFO, Error>` using [`ParamSFO::from_parts`].
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
///     "STRINGKEY" => ["Value", 8],
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
