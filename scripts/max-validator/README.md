# Max Runtime Validator

Validates `.maxpat` files by loading them inside a running Max instance. This catches issues that static analysis cannot detect (e.g., missing externals, invalid object arguments).

## Requirements

- **Max 9** (or later)
- The validator patch must be open in Max while running validation

## Setup

1. Open `flutmax-validator.maxpat` in Max
2. The patch auto-starts a UDP server on ports 7401 (receive) / 7402 (send)

## Usage

With the validator patch running in Max:

```bash
flutmax validate --max patch.maxpat
```

The CLI sends the file path to Max via UDP, Max loads and inspects the patch, then returns validation results.

## Architecture

```
CLI  --UDP 7401-->  [node.script]  --outlet-->  [v8.codebox]  --File+JSON-->  validate
CLI  <--UDP 7402--  [node.script]  <--outlet--  [v8.codebox]  <--result JSON
```

- **node.script** — Node for Max UDP server (`flutmax-validate-server.mjs`)
- **v8.codebox** — Patch inspector (`flutmax-inspect.js`): reads .maxpat, validates JSON structure, box fields, ID uniqueness, and connection validity

## Files

| File | Role |
|------|------|
| `flutmax-validator.maxpat` | Max patch (open this in Max) |
| `flutmax-validator.flutmax` | Source for the validator patch |
| `flutmax-validate-server.mjs` | Node for Max UDP server |
| `flutmax-inspect.js` | v8.codebox validation logic |

## Checks Performed

- JSON structure (patcher root, boxes array)
- Required box fields (id, maxclass, numinlets, numoutlets, patching_rect)
- Box ID uniqueness
- newobj boxes have text field
- Connection validity (source/destination box IDs exist, inlet/outlet indices in range)
