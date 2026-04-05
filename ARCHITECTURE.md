# flutmax Architecture

- **Updated**: 2026-04-05

## Overview

flutmax is a transpiler for writing Max/MSP patch files (`.maxpat`) as structured text (`.flutmax`). It is implemented as a Rust workspace of 10 crates, providing bidirectional conversion: compile (`.flutmax` to `.maxpat`) and decompile (`.maxpat` to `.flutmax` + `.uiflutmax`).

### Design Principles

- **Bidirectional Fidelity**: Semantic information is preserved through decompile → compile roundtrips (verified at 100% across 1282 patches)
- **Deterministic Execution Order**: Replaces Max's coordinate-dependent execution order with code order (top to bottom), automatically inserting `trigger` objects
- **Type Safety**: Signal / Control connection mismatches detected at compile time. Control is further subdivided into Int / Float / Symbol / Bang / List
- **Git Friendliness**: Text-based with clear diffs. Flat structure where 1 file = 1 Abstraction. Visual attributes separated into `.uiflutmax` sidecar

## High-Level Overview

```mermaid
graph TB
    FLUTMAX[".flutmax Source"] --> PARSER
    MAXPAT_IN[".maxpat (Existing Patch)"] --> DECOMPILE

    subgraph "Compile Pipeline"
        PARSER["flutmax-parser<br/>Lexer + Recursive Descent → AST"]
        SEMA["flutmax-sema<br/>Type Check / Trigger Insertion /<br/>Abstraction Registry"]
        CODEGEN["flutmax-codegen<br/>AST → PatchGraph → Sugiyama Layout → JSON"]
    end

    subgraph "Decompile Pipeline"
        DECOMPILE["flutmax-decompile<br/>.maxpat JSON → .flutmax + .uiflutmax"]
    end

    subgraph "Shared Libraries"
        AST["flutmax-ast<br/>AST Type Definitions"]
        OBJDB["flutmax-objdb<br/>Max Object Database<br/>(1573 objects)"]
        VALIDATE["flutmax-validate<br/>Static + Runtime Validation"]
    end

    subgraph "IDE / Bindings"
        LSP["flutmax-lsp<br/>Language Server Protocol"]
        VSCODE["editors/vscode<br/>LSP Client + TextMate Grammar"]
        UNIFIED["flutmax (crate)<br/>Unified Facade"]
        PY["bindings/python"]
        WASM["bindings/wasm"]
    end

    CLI["flutmax-cli<br/>compile / decompile / validate"]

    PARSER --> AST
    AST --> SEMA
    SEMA --> CODEGEN
    CODEGEN --> MAXPAT_OUT[".maxpat (Generated)"]
    DECOMPILE --> FLUTMAX_OUT[".flutmax + .uiflutmax"]

    OBJDB --> SEMA
    OBJDB --> CODEGEN
    OBJDB --> DECOMPILE
    OBJDB --> VALIDATE
    OBJDB --> LSP
    MAXPAT_OUT --> VALIDATE

    CLI -.-> PARSER
    CLI -.-> CODEGEN
    CLI -.-> DECOMPILE
    CLI -.-> VALIDATE

    LSP -.-> PARSER
    LSP -.-> SEMA

    UNIFIED --> PARSER
    UNIFIED --> CODEGEN
    UNIFIED --> DECOMPILE
    PY --> UNIFIED
    WASM --> UNIFIED
```

## Crate Dependency Graph

```mermaid
graph TD
    CLI["flutmax-cli"] --> PARSER["flutmax-parser"]
    CLI --> CODEGEN["flutmax-codegen"]
    CLI --> SEMA["flutmax-sema"]
    CLI --> VALIDATE["flutmax-validate"]
    CLI --> DECOMPILE["flutmax-decompile"]
    CLI --> OBJDB["flutmax-objdb"]
    CLI --> AST["flutmax-ast"]

    UNIFIED["flutmax"] --> PARSER
    UNIFIED --> CODEGEN
    UNIFIED --> SEMA
    UNIFIED --> DECOMPILE
    UNIFIED --> OBJDB
    UNIFIED --> AST

    LSP["flutmax-lsp"] --> PARSER
    LSP --> AST
    LSP --> SEMA
    LSP --> OBJDB
    LSP --> VALIDATE

    PARSER --> AST
    SEMA --> AST
    CODEGEN --> AST
    CODEGEN --> SEMA
    CODEGEN --> OBJDB
    DECOMPILE --> OBJDB
    VALIDATE --> OBJDB

    style AST fill:#e1f5fe
    style OBJDB fill:#e8f5e9
    style DECOMPILE fill:#fff3e0
    style LSP fill:#fce4ec
    style UNIFIED fill:#f3e5f5
```

## Crate Overview

### Compile Pipeline

| Crate | Role | Input | Output |
|-------|------|-------|--------|
| **flutmax-parser** | Lexer + recursive descent parser (pure Rust) | Source string | `Program` (AST) |
| **flutmax-sema** | Type checking (Signal/Control), trigger insertion, Abstraction registry | AST + Registry | Type errors / PatchGraph extensions |
| **flutmax-codegen** | AST → PatchGraph → .maxpat JSON with Sugiyama graph layout | AST + Registry + ObjDb | .maxpat JSON string |

### Decompile Pipeline

| Crate | Role | Input | Output |
|-------|------|-------|--------|
| **flutmax-decompile** | Analyze .maxpat, remove triggers, name wires, separate UI | .maxpat JSON + ObjDb | .flutmax source + .uiflutmax sidecar |

### Shared Libraries

| Crate | Role |
|-------|------|
| **flutmax-ast** | AST type definitions (`Program`, `Wire`, `Expr`, `CallArg`, `Span`, etc.) |
| **flutmax-objdb** | Max object database from `refpages/*.maxref.xml` — 1573 objects with inlet/outlet count, types, Hot/Cold, descriptions |
| **flutmax-validate** | Static .maxpat validation (JSON structure + objdb checks) and optional Max runtime validation via Node for Max UDP |

### Entry Points

| Crate | Role |
|-------|------|
| **flutmax-cli** | CLI with `compile` / `decompile` / `validate` subcommands |
| **flutmax-lsp** | Language Server Protocol — diagnostics, completion (1573 objects), hover (inlet details), go-to-definition, semantic tokens, signature help |
| **flutmax** | Unified facade crate — `compile()`, `decompile()`, `parse_to_json()` for library use |

### Bindings

| Binding | Technology | API |
|---------|-----------|-----|
| **bindings/python** | PyO3 / maturin | `flutmax_py.compile()`, `.decompile()`, `.parse()` |
| **bindings/wasm** | wasm-bindgen / wasm-pack | `compile()`, `decompile()`, `parse()` |

### External Components

| Component | Language | Role |
|-----------|----------|------|
| **tree-sitter-flutmax** | JavaScript (grammar.js) → C (parser.c) | Grammar definition for VS Code syntax highlighting only |
| **editors/vscode** | TypeScript | VS Code extension — LSP client, TextMate grammar, snippets |
| **scripts/max-validator** | JavaScript (v8.codebox + Node for Max) | Runtime validation server inside Max (UDP 7401/7402) |

## Compile Pipeline

```mermaid
sequenceDiagram
    participant User
    participant CLI as flutmax-cli
    participant Parser as flutmax-parser
    participant Sema as flutmax-sema
    participant Codegen as flutmax-codegen
    participant FS as File System

    User->>CLI: flutmax compile src/ -o dist/
    CLI->>FS: Enumerate .flutmax files

    loop All files (Phase A: Registration)
        CLI->>Parser: parse(source)
        Parser-->>CLI: Program (AST)
        CLI->>Sema: registry.register(name, ast)
    end

    loop All files (Phase B: Compilation)
        CLI->>Sema: type_check_with_registry(ast, registry)
        Sema-->>CLI: Vec<TypeError>
        CLI->>Codegen: build_graph(ast, registry, objdb)
        Note over Codegen: Trigger auto-insertion<br/>Named arg → inlet resolution<br/>Hot/cold + purity classification
        Codegen-->>CLI: PatchGraph
        CLI->>Codegen: generate(graph) with Sugiyama layout
        Codegen-->>CLI: .maxpat JSON
        CLI->>FS: Write .maxpat
    end
```

## Decompile Pipeline

```mermaid
sequenceDiagram
    participant User
    participant CLI as flutmax-cli
    participant Decompile as flutmax-decompile
    participant FS as File System

    User->>CLI: flutmax decompile --multi input.maxpat -o output/
    CLI->>FS: Read .maxpat
    CLI->>Decompile: decompile_multi(json, name, objdb)

    Note over Decompile: 1. JSON parse (boxes, lines, attrs)
    Note over Decompile: 2. Recursive subpatcher expansion
    Note over Decompile: 3. Trigger removal + rewiring
    Note over Decompile: 4. Comment/panel/image → .uiflutmax
    Note over Decompile: 5. Topological sort
    Note over Decompile: 6. Wire naming (object-name based)
    Note over Decompile: 7. Wire expression + named args from objdb
    Note over Decompile: 8. Decorative attrs → .uiflutmax, functional attrs → .attr()
    Note over Decompile: 9. .flutmax text output

    Decompile-->>CLI: DecompileResult (.flutmax + .uiflutmax + code files)
    CLI->>FS: Write files
```

## AST Structure

```mermaid
classDiagram
    class Program {
        +Vec~InDecl~ in_decls
        +Vec~OutDecl~ out_decls
        +Vec~Wire~ wires
        +Vec~DestructuringWire~ destructuring_wires
        +Vec~OutAssignment~ out_assignments
        +Vec~DirectConnection~ direct_connections
        +Vec~FeedbackDecl~ feedback_decls
        +Vec~FeedbackAssignment~ feedback_assignments
        +Vec~StateDecl~ state_decls
        +Vec~MsgDecl~ msg_decls
    }

    class Wire {
        +String name
        +Expr value
        +Option~Span~ span
        +Vec~AttrPair~ attrs
    }

    class CallArg {
        +Option~String~ name
        +Expr value
    }

    class Expr {
        <<enumeration>>
        Call(object, Vec~CallArg~)
        Ref(name)
        Lit(LitValue)
        OutputPortAccess(object, index)
        Tuple(Vec~Expr~)
    }

    Program --> Wire
    Wire --> Expr
    Expr --> CallArg
    Wire --> AttrPair
```

## PatchGraph (IR)

```mermaid
classDiagram
    class PatchGraph {
        +Vec~PatchNode~ nodes
        +Vec~PatchEdge~ edges
    }

    class PatchNode {
        +String id
        +String object_name
        +Vec~String~ args
        +u32 num_inlets
        +u32 num_outlets
        +bool is_signal
        +Option~String~ varname
        +Vec~bool~ hot_inlets
        +NodePurity purity
        +Vec~(String,String)~ attrs
    }

    class PatchEdge {
        +String source_id
        +u32 source_outlet
        +String dest_id
        +u32 dest_inlet
        +bool is_feedback
        +Option~u32~ order
    }

    PatchGraph --> PatchNode
    PatchGraph --> PatchEdge
```

## Directory Structure

```
flutmax/
├── Cargo.toml                    # Workspace root
├── README.md                     # Project overview
├── SYNTAX.md                     # Language syntax specification
├── ARCHITECTURE.md               # This document
├── LICENSE                       # MIT
│
├── crates/
│   ├── flutmax/                  # Unified facade crate (compile, decompile, parse_to_json)
│   ├── flutmax-ast/              # AST type definitions (Expr, Wire, Program, Span, CallArg)
│   ├── flutmax-parser/           # Hand-written Pure Rust lexer + recursive descent parser
│   ├── flutmax-sema/             # Semantic analysis (type check, trigger insertion, registry)
│   ├── flutmax-codegen/          # AST → PatchGraph → .maxpat JSON (Sugiyama layout)
│   ├── flutmax-objdb/            # Max object database (1573 objects from refpages)
│   ├── flutmax-validate/         # Static validation + Node for Max runtime validation
│   ├── flutmax-decompile/        # .maxpat → .flutmax + .uiflutmax decompiler
│   ├── flutmax-lsp/              # Language Server Protocol (diagnostics, completion, hover, go-to-def)
│   └── flutmax-cli/              # CLI entry point (compile / decompile / validate)
│
├── bindings/
│   ├── python/                   # Python bindings (PyO3 / maturin)
│   └── wasm/                     # WebAssembly bindings (wasm-bindgen / wasm-pack)
│
├── editors/
│   └── vscode/                   # VS Code extension (syntax, snippets, LSP client)
│
├── tree-sitter-flutmax/          # Tree-sitter grammar (VS Code syntax highlighting)
│
├── examples/
│   └── synths/                   # Synthesizer examples (FM, subtractive, delay, granular)
│
├── scripts/
│   └── max-validator/            # Max runtime validator (v8.codebox + Node for Max UDP)
│
└── tests/
    └── e2e/                      # End-to-end test fixtures and expected outputs
```

## Test Suite

| Category | Count | Description |
|----------|-------|-------------|
| Rust unit/integration tests | 915+ | Tests across all crates |
| VS Code extension tests | 86 | Grammar, snippets, language-config, package |
| Tree-sitter corpus tests | ~70 | Parser syntax tests |
| Max roundtrip tests | 1282 | Max.app patch decompile → compile → compare (100% PASS) |
| **Total** | **~2353** | |
