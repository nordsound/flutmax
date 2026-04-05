# flutmax-parser

Parser for .flutmax files (hand-written lexer + recursive descent).

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

A pure Rust parser with no external grammar dependencies. Includes a hand-written lexer and recursive-descent parser that produces an `flutmax_ast::Program` AST. A legacy Tree-sitter backend is available behind the `tree-sitter-legacy` feature flag.

## Usage

```rust
use flutmax_parser::parse;

let source = "wire osc = cycle~(440);";
let ast = parse(source).expect("parse error");
```

## Features

- `tree-sitter-legacy` -- use the Tree-sitter grammar instead of the hand-written parser

## License

MIT
