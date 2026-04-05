# Synthesizer Examples

Real-world synthesizer patches written in `.flutmax`. Each patch compiles and validates successfully.

## Patches

### Single File

| File | Description | Techniques |
|---|---|---|
| `fm_synth.flutmax` | 2-operator FM synthesis | Control/Signal mixing, `mul` / `mul~` / `cycle~` |
| `subtractive_synth.flutmax` | Subtractive synthesis | `phasor~`, `biquad~` filter, dual oscillators |
| `delay_effect.flutmax` | Feedback delay | `feedback` keyword, `tapin~` / `tapout~`, dry/wet mix |
| `granular_simple.flutmax` | Simple granular texture | 4-oscillator bank, detune, normalization |

### Multi-File (`multi_file_synth/`)

| File | Description |
|---|---|
| `oscillator.flutmax` | Sine wave oscillator (Abstraction) |
| `mixer_2ch.flutmax` | 2-channel mixer (Abstraction) |
| `main_synth.flutmax` | Main patch — references oscillator and mixer_2ch |

## Compile

```bash
# Single file
cargo run -p flutmax-cli -- compile examples/synths/fm_synth.flutmax -o dist/fm_synth.maxpat

# Multi-file (directory)
cargo run -p flutmax-cli -- compile examples/synths/multi_file_synth/ -o dist/multi_file/

# Validate
cargo run -p flutmax-cli -- validate --ci dist/fm_synth.maxpat
```

## Test

```bash
cargo test -p flutmax-cli --test synth_examples
```
