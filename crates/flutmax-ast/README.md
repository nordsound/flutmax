# flutmax-ast

AST type definitions for the flutmax language.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Defines the core Abstract Syntax Tree types shared across the flutmax compiler pipeline: `Program`, `InDecl`, `OutDecl`, `Wire`, `OutAssignment`, `Expr`, and related node types.

## Usage

```rust
use flutmax_ast::{Program, Wire, Expr};

let program = Program {
    in_decls: vec![],
    out_decls: vec![],
    wires: vec![],
    out_assignments: vec![],
    nodes: vec![],
};
```

## License

MIT
