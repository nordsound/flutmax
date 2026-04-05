# flutmax

A transpiler between `.flutmax` text and Max/MSP `.maxpat` patch files.

Write Max patches as structured, version-controllable code. Decompile existing patches into readable text. Compile text back into working Max patches.

## The Problem

Max/MSP patches are stored as opaque JSON with coordinates, colors, and connection indices tangled together. This makes them:

- **Impossible to diff** — a one-wire change produces hundreds of lines of JSON noise
- **Unreviewable** — no one can code-review a `.maxpat` in a pull request
- **Order-dependent** — execution order is determined by invisible X/Y coordinates
- **Hard to refactor** — renaming or restructuring means clicking through visual spaghetti

## The Solution

flutmax introduces `.flutmax`, a text format that aims to represent Max patches as readable code:

```flutmax
// FM synthesizer — carrier + modulator with depth control
in carrier_freq: float;
in harmonicity: float;
in mod_index: float;

wire mod_freq = mul~(carrier_freq, harmonicity);
wire modulator = cycle~(mod_freq);
wire mod_depth = mul~(modulator, mod_index);
wire carrier = cycle~(add~(carrier_freq, mod_depth));

out fm_signal: signal = carrier;
```

This compiles to a fully working `.maxpat` that you can open in Max. And any `.maxpat` can be decompiled back to `.flutmax`.

## How It Works

### Compilation (.flutmax → .maxpat)

```
.flutmax source
    ↓ Lexer (tokenize)
    ↓ Parser (recursive descent → AST)
    ↓ Semantic analysis (type check Signal/Control, detect fanout)
    ↓ Trigger insertion (auto-insert [trigger] for deterministic ordering)
    ↓ Graph layout (Sugiyama layered algorithm)
    ↓ JSON generation
.maxpat file (open in Max)
```

The compiler:
1. **Parses** `.flutmax` into an AST (pure Rust, no C dependencies)
2. **Type-checks** Signal vs Control connections — catches wiring errors before you open Max
3. **Auto-inserts `trigger` objects** — Max normally relies on X-coordinate ordering for fanout, which is fragile. flutmax makes execution order explicit and deterministic based on code order
4. **Lays out the patch** using the Sugiyama algorithm — inlets at top, outlets at bottom, signal flow visible
5. **Generates** standard `.maxpat` JSON that Max reads natively

### Decompilation (.maxpat → .flutmax)

```
.maxpat file
    ↓ JSON parse
    ↓ Box classification (inlet/outlet/wire/message/comment/panel)
    ↓ Trigger removal (reverse the auto-insertion)
    ↓ Topological sort (determine code order from graph)
    ↓ Wire naming (object-name based: cycle~→cycle, biquad~→biquad)
    ↓ UI separation (decorative attrs → .uiflutmax sidecar)
.flutmax source + .uiflutmax layout
```

The decompiler:
1. **Classifies** every Max box — inlets/outlets become port declarations, objects become wires, comments and panels go to `.uiflutmax`
2. **Removes trigger objects** that Max uses for ordering (flutmax re-inserts them during compilation)
3. **Names wires** from object names (`cycle~` → `cycle`, `biquad~` → `biquad`) instead of opaque IDs
4. **Separates concerns** — logic goes to `.flutmax`, positions and decorative attributes go to `.uiflutmax`

### The .uiflutmax Sidecar

```
patch.flutmax          ← Logic (what you edit and review)
patch.uiflutmax        ← Layout + styling (JSON, optional)
codebox_1.js           ← v8.codebox code (if any)
```

- **Without `.uiflutmax`** — the compiler uses automatic Sugiyama graph layout
- **With `.uiflutmax`** — original positions, colors, and visual elements are restored
- **Git workflow** — logic changes appear in `.flutmax` diffs, layout changes in `.uiflutmax` diffs

The `.uiflutmax` also stores non-logic visual elements:

```json
{
  "_patcher": { "rect": [100, 100, 640, 480] },
  "_comments": [{ "text": "oscillator section", "rect": [50, 30, 200, 20] }],
  "_panels": [{ "rect": [40, 20, 400, 200] }],
  "cycle": { "rect": [150, 200, 80, 22] },
  "filter": { "rect": [200, 300, 80, 22] }
}
```

## Install

```bash
cargo install flutmax
```

## Usage

```bash
# Compile a single file
flutmax compile synth.flutmax -o synth.maxpat

# Compile a directory (multi-file Abstraction support)
flutmax compile src/ -o dist/

# Decompile (single file)
flutmax decompile patch.maxpat -o patch.flutmax

# Decompile with subpatchers, codebox code, and UI layout
flutmax decompile --multi complex_patch.maxpat -o output/main.flutmax

# Validate a .maxpat (static checks)
flutmax validate patch.maxpat

# Validate via Max runtime (requires Max 9 with validator patch running)
# See scripts/max-validator/README.md for setup instructions
flutmax validate --max patch.maxpat
```

## Syntax Reference

### Ports

```flutmax
in freq: float;              // Input port (implicit index from declaration order)
in 2 (special): signal;      // Explicit index (when order matters)
out audio: signal = osc;     // Output with inline assignment
out[0] = osc;                // Index-based assignment (backward compatible)
```

Port types: `signal`, `float`, `int`, `bang`, `list`, `symbol`

### Wires

```flutmax
wire osc = cycle~(440);                      // Create object, connect args to inlets
wire filter = biquad~(osc);                  // Reference another wire
wire mix = add~(saw, mul~(noise, 0.1));      // Inline nesting
wire (key, velocity) = unpack(midi_input);   // Destructuring (multiple outlets)
```

### Connections

```flutmax
filter.in[1] = cutoff;       // Connect to specific inlet
filter.in[2] = resonance;    // Multiple inlets
source.out[1]                // Reference specific outlet (outlet 0 is default)
```

### Messages and Attributes

```flutmax
msg setdomain = "setdomain $1";                           // Message box
wire dial = live.dial().attr(min: 0, max: 127);           // Object attributes
wire env = function().attr(domain: 1000.0, range: "0 1"); // Functional attributes stay in .flutmax
```

### Special Constructs

```flutmax
state counter: int = 0;                   // Stateful object ([int] / [float])
feedback tap = tapin~(input);             // Feedback loop (tapin~/tapout~)
wire delay = tapout~(tap, 500);           // tapout~ reads from tapin~ buffer
```

## Language Bindings

flutmax is Pure Rust with no C dependencies, making cross-platform bindings straightforward.

### Rust

```rust
// Unified crate — one dependency for everything
let maxpat = flutmax::compile("wire osc = cycle~(440);\nout audio: signal = osc;").unwrap();
let source = flutmax::decompile(&maxpat).unwrap();

// Or use sub-crates for fine-grained control
let ast = flutmax::parser::parse(source)?;
let graph = flutmax::codegen::build_graph(&ast)?;
```

### Python

```bash
cd bindings/python && maturin develop
```

```python
import flutmax_py
import json

maxpat = flutmax_py.compile("wire osc = cycle~(440);\nout audio: signal = osc;")
source = flutmax_py.decompile(maxpat)
ast = json.loads(flutmax_py.parse("wire osc = cycle~(440);"))
```

### WASM / JavaScript

```bash
cd bindings/wasm && wasm-pack build --target web
```

```javascript
import init, { compile, decompile, parse } from './flutmax_wasm.js';
await init();

const maxpat = compile("wire osc = cycle~(440);\nout audio: signal = osc;");
```

## IDE Support

The LSP server provides full language intelligence in any editor that supports LSP (VS Code, Neovim, Helix, Zed, Emacs, etc.):

- **Diagnostics** — parse errors and type errors with error recovery (reports all errors, not just the first)
- **Completion** — keywords, defined wire names, and 1573 Max objects from the built-in object database
- **Hover** — object description, inlet/outlet names, types, and descriptions from Max refpages
- **Go to Definition** — Ctrl+click on a wire reference to jump to its declaration
- **Semantic Highlighting** — keywords, Max objects, wire names, types colored differently
- **Signature Help** — parameter info popup when typing `(`

```bash
# Build the LSP server
cargo build --release -p flutmax-lsp

# VS Code: open editors/vscode/ and press F5
# Neovim: add to lspconfig with cmd = { "path/to/flutmax-lsp" }
# Helix: add to languages.toml
```

## Architecture

```
Compile:    .flutmax → Lexer → Parser → AST → Sema → PatchGraph → Layout → .maxpat
Decompile:  .maxpat → JSON parse → classify → sort → name → emit → .flutmax + .uiflutmax
```

| Crate | Role |
|-------|------|
| `flutmax` | Unified facade crate |
| `flutmax-parser` | Hand-written lexer + recursive descent parser |
| `flutmax-ast` | AST type definitions |
| `flutmax-sema` | Type checking, trigger insertion, abstraction registry |
| `flutmax-codegen` | AST → PatchGraph → .maxpat JSON (Sugiyama layout) |
| `flutmax-decompile` | .maxpat → .flutmax + .uiflutmax |
| `flutmax-objdb` | Max object database (1573 objects from refpages) |
| `flutmax-validate` | Static validation + Node for Max runtime validation |
| `flutmax-lsp` | Language Server Protocol (diagnostics, completion, hover, go-to-def, semantic tokens) |
| `flutmax-cli` | CLI entry point |

## Supported Max Features

| Feature | Details |
|---------|---------|
| Max/MSP objects | 1573 objects in database (Max, MSP, Jitter, M4L, 18 packages) |
| Subpatchers | `p`, `poly~`, `pfft~` decompiled as separate files |
| gen~ | Visual patchers with `classnamespace: dsp.gen` |
| RNBO | `classnamespace: rnbo`, `inport`/`outport` recognition |
| v8.codebox / gen~ codebox | Code extracted to `.js` / `.genexpr` files |
| Trigger auto-insertion | Value-preserving types (`f`/`i`, not `b`), signal paths excluded |
| Template arguments | `#N`, `$fN` preserved as string literals |
| Feedback loops | `tapin~`/`tapout~` cycle detection and resolution |
| Parameter system | `pattr`, `autopattr`, `varname` preserved |
| Auto layout | Sugiyama layered graph drawing (inlet→outlet flow) |
| UI separation | Decorative attrs, positions, comments, panels → `.uiflutmax` |

## Testing

```bash
# Run all Rust tests
cargo test --workspace

# Run VS Code extension integration tests
cd editors/vscode && npm install && npx tsc && npm test

# Roundtrip verification (requires Max.app installed)
# 1018 Max reference patches + 264 real-world patches
cargo test -p flutmax-cli --test max_reference_roundtrip
```

## License

MIT License
