# tree-sitter-flutmax

Tree-sitter grammar for the flutmax DSL.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Defines the Tree-sitter grammar for `.flutmax` files. Used primarily for VS Code syntax highlighting via the [flutmax VS Code extension](https://github.com/nordsound/flutmax/tree/main/editors/vscode). The hand-written parser in `flutmax-parser` is used for compilation; this grammar provides editor integration.

## Development

```bash
# Generate the parser from grammar.js
npx tree-sitter generate

# Run grammar tests
npx tree-sitter test
```

## License

MIT
