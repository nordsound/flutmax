# flutmax-objdb

Max object definition database parsed from .maxref.xml refpages.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Provides a compiled database of 1573 Max/MSP objects scraped from the official `refpages/*.maxref.xml` files. Each entry contains inlet/outlet count, port types (signal/float/int), and hot/cold attributes. The database is embedded at compile time for zero-cost lookups.

## Usage

```rust
use flutmax_objdb::ObjectDb;

let db = ObjectDb::default();
if let Some(obj) = db.lookup("cycle~") {
    println!("inlets: {}", obj.inlets.len());
}
```

## License

MIT
