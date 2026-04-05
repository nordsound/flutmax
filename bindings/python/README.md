# flutmax-py

Python bindings for flutmax (.flutmax <-> .maxpat transpiler).

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Install

```bash
pip install maturin
maturin develop --release
```

## Usage

```python
import flutmax_py

# Compile .flutmax source to .maxpat JSON
maxpat = flutmax_py.compile("out audio: signal;\nwire osc = cycle~(440);\nout[0] = osc;")

# Decompile .maxpat JSON to .flutmax source
source = flutmax_py.decompile(maxpat)

# Parse to AST JSON
ast = flutmax_py.parse("wire osc = cycle~(440);")
```

## License

MIT
