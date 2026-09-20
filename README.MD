# param_sfo: read and write PARAM.SFO files for PS3

This library lets you create and modify `PARAM.SFO` files for PS3.
SFO (System File Object) files store metadata for content displayed in the XMB,
saved games, and trophies. For more information, see the
[format description](https://www.psdevwiki.com/ps3/PARAM.SFO).

## Usage

Entries are stored in a `BTreeMap<Key, Data>`, which keeps the keys in
alphabetical order, as required by the format. Keys must also be uppercase;
the `Key` struct handles this conversion automatically.

### Reading and modifying an existing SFO

Use the `from_reader`, `from_file`, or `from_bytes` methods of `ParamSFO`
to read an existing file. Then use the `entries` and `entries_mut` methods
to access and modify its entries.

The following example opens `PARAM.SFO`, adds a new entry,
and then saves it as `PARAM_modified.SFO`.

```no_run
use param_sfo::{ParamSFO, Data, Key};
let mut sfo = ParamSFO::from_file("./tests/PARAM.SFO")?;
sfo.entries_mut().insert(
    Key::new("NEWKEY")?,
    Data::new("Some new value".into(), 32)?
);
sfo.to_file("./PARAM_modified.SFO")?;
# Ok::<(), param_sfo::Error>(())
```

### Creating a new SFO from scratch

You can create a new SFO using the `param_sfo!` macro:

```rust
use param_sfo::{ParamSFO, Error, param_sfo};

let key = "MYKEY";
let value = "Hello world!";

let sfo: Result<ParamSFO, Error> = param_sfo! {
    "STRINGKEY" => ["Value", 8],
    "BYTESKEY" => [vec![1,2,3], 8],
    "INTKEY" => [123, 4],
    key => [value, 20]
};
```

Alternatively, you can create one manually:

```rust
use param_sfo::{ParamSFO, Key, Data};
use std::collections::BTreeMap;

let mut entries = BTreeMap::new();
let key = Key::try_from("MYKEY")?;
let value = Data::new("Hello world!".into(), 32)?;
entries.insert(key, value);
let sfo = ParamSFO::with_entries(entries);
# Ok::<(), param_sfo::Error>(())
```
