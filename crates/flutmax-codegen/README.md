# flutmax-codegen

Code generator: analyzed graph -> .maxpat JSON.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Transforms the analyzed AST into a `PatchGraph` and serializes it as a `.maxpat` JSON file compatible with Max/MSP. Uses a Sugiyama-style layered layout algorithm for automatic object placement.

## Usage

```rust
let ast = flutmax_parser::parse(source).unwrap();
let graph = flutmax_codegen::build_graph(&ast).unwrap();
let maxpat_json = flutmax_codegen::generate(&graph).unwrap();
```

## License

MIT
