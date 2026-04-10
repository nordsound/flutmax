# flutmax

Python bindings for the [flutmax](https://github.com/nordsound/flutmax) transpiler — convert between `.flutmax` text and Max/MSP `.maxpat` patches.

## Install

```bash
pip install flutmax
```

A prebuilt wheel is currently available for **macOS (arm64)**. On other
platforms (Linux, Windows, macOS x86_64), `pip` falls back to building
from source — see [Building from source](#building-from-source) below.

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

## Building from source

When no prebuilt wheel is available for your platform, `pip install flutmax`
will attempt to compile from the source distribution. This requires:

| Dependency | How to install |
| --- | --- |
| **Rust toolchain** (stable) | `curl https://sh.rustup.rs -sSf \| sh` |
| **C compiler** | Linux: `apt install build-essential` / `dnf install gcc` · Windows: Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "C++ build tools" workload · macOS: `xcode-select --install` |
| **Python development headers** | Linux: `apt install python3-dev` / `dnf install python3-devel` · macOS/Windows: included with the standard Python installer |

Once the prerequisites are in place, `pip install flutmax` should succeed
automatically (maturin is pulled in as the build backend).

To build a wheel manually:

```bash
git clone https://github.com/nordsound/flutmax.git
cd flutmax/bindings/python
pip install maturin
maturin build --release        # wheel is written to target/wheels/
pip install target/wheels/*.whl
```

### Development build (editable install)

```bash
cd flutmax/bindings/python
pip install maturin
maturin develop --release      # builds and installs in the current venv
python -c "import flutmax_py; print(flutmax_py.__version__)"
```

## License

MIT — see [LICENSE](https://github.com/nordsound/flutmax/blob/main/LICENSE)
