# flutmax

Transpiler between .flutmax text and Max/MSP .maxpat patches.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Unified facade crate that re-exports the entire flutmax compiler pipeline. Provides high-level `compile()`, `decompile()`, and `parse_to_json()` functions for common workflows.

## Usage

```rust
// Compile .flutmax source to .maxpat JSON
let maxpat = flutmax::compile("out audio: signal;\nwire osc = cycle~(440);\nout[0] = osc;").unwrap();

// Decompile .maxpat JSON back to .flutmax source
let source = flutmax::decompile(&maxpat).unwrap();

// Parse to AST and return as JSON (useful for bindings)
let ast_json = flutmax::parse_to_json("wire osc = cycle~(440);").unwrap();
```

## Sub-crate access

For advanced usage, sub-crates are re-exported:

```rust
use flutmax::ast;
use flutmax::parser;
use flutmax::sema;
use flutmax::codegen;
use flutmax::objdb;
use flutmax::decompile;
```

## License

MIT
