# flutmax

Python bindings for the [flutmax](https://github.com/nordsound/flutmax) transpiler — convert between `.flutmax` text and Max/MSP `.maxpat` patches.

## Install

```bash
pip install flutmax
```

## Usage

```python
import flutmax_py

# Compile .flutmax source to .maxpat JSON
maxpat = flutmax_py.compile("wire osc = cycle~(440);\nout audio: signal = osc;")

# Decompile .maxpat JSON to .flutmax source
source = flutmax_py.decompile(maxpat)

# Parse to AST JSON
import json
ast = json.loads(flutmax_py.parse("wire osc = cycle~(440);"))
```

## License

MIT — see [LICENSE](https://github.com/nordsound/flutmax/blob/main/LICENSE)
