# flutmax-sema

Semantic analysis: type checking, trigger insertion, abstraction registry.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Performs semantic analysis on the parsed AST, including:

- **Type checking** -- signal vs. control type validation
- **Trigger insertion** -- identifies fan-out points requiring automatic `[trigger]` objects
- **Abstraction registry** -- resolves references to external `.flutmax` abstractions

## Usage

```rust
use flutmax_sema::analyze;

let ast = flutmax_parser::parse(source).unwrap();
let analyzed = analyze(&ast).expect("semantic error");
```

## License

MIT
