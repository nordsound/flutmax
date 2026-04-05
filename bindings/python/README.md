# flutmax

Python bindings for the [flutmax](https://github.com/nordsound/flutmax) transpiler — convert between `.flutmax` text and Max/MSP `.maxpat` patches.

## Install

```bash
pip install flutmax
```

## Usage

### File-based (compile / decompile files directly)

```python
import flutmax_py

# Compile .flutmax → .maxpat
flutmax_py.compile_file("synth.flutmax", "synth.maxpat")

# Decompile .maxpat → .flutmax
flutmax_py.decompile_file("patch.maxpat", "patch.flutmax")
```

### String-based (for programmatic use)

```python
import flutmax_py
import json

# Compile source string to .maxpat JSON
maxpat = flutmax_py.compile("wire osc = cycle~(440);\nout audio: signal = osc;")

# Decompile .maxpat JSON to .flutmax source
source = flutmax_py.decompile(maxpat)

# Parse to AST JSON
ast = json.loads(flutmax_py.parse("wire osc = cycle~(440);"))
```

## License

MIT — see [LICENSE](https://github.com/nordsound/flutmax/blob/main/LICENSE)
