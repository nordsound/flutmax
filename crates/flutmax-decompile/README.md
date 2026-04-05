# flutmax-decompile

.maxpat JSON to .flutmax source decompiler.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Converts existing Max/MSP `.maxpat` JSON files back into `.flutmax` text. Supports multi-file output for subpatchers, codebox content, and UI layout data (`.uiflutmax`).

## Usage

```rust
let maxpat_json = std::fs::read_to_string("patch.maxpat").unwrap();
let source = flutmax_decompile::decompile(&maxpat_json).unwrap();

// Multi-file decompile (subpatchers, UI data)
let result = flutmax_decompile::decompile_multi(&maxpat_json, "patch").unwrap();
```

## License

MIT
