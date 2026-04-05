# flutmax-wasm

WASM bindings for flutmax (.flutmax <-> .maxpat transpiler).

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Build

```bash
wasm-pack build --target web
```

## Usage

```javascript
import init, { compile, decompile, parse } from "./pkg/flutmax_wasm.js";

await init();

// Compile .flutmax source to .maxpat JSON
const maxpat = compile("out audio: signal;\nwire osc = cycle~(440);\nout[0] = osc;");

// Decompile .maxpat JSON to .flutmax source
const source = decompile(maxpat);

// Parse to AST JSON
const ast = parse("wire osc = cycle~(440);");
```

## License

MIT
