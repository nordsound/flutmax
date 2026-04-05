# flutmax-cli

CLI entry point for the flutmax transpiler.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Usage

```bash
# Compile .flutmax to .maxpat
flutmax compile src/synth.flutmax -o dist/synth.maxpat

# Decompile .maxpat to .flutmax
flutmax decompile patch.maxpat -o src/patch.flutmax

# Validate a .maxpat file
flutmax validate dist/synth.maxpat
```

## Install

```bash
cargo install --path .
```

## License

MIT
