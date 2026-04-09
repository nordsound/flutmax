# flutmax-sim

Pure-Rust gen~ and RNBO DSP simulator for headless audio testing.

Part of the [flutmax](https://github.com/nordsound/flutmax) workspace.

## Overview

Reads compiled `.maxpat` JSON and executes the signal graph sample-by-sample,
so flutmax patches can be verified in CI without launching Max/MSP. Covers the
gen~ operator set (~60 ops), RNBO `param`/`notein`/`in~`/`out~`/embedded gen~,
and provides audio analysis utilities (`peak`, `rms`, `freq_estimate`,
`is_silent`) for assertion-style tests.

Used by the `flutmax sim` CLI subcommand to assert audio properties of compiled
patches end-to-end.

## Usage

### gen~ patcher

```rust
use flutmax_sim::GenSimulator;

let json = std::fs::read_to_string("simple.maxpat").unwrap();
let mut sim = GenSimulator::from_json_with_sr(&json, 48_000.0).unwrap();

sim.set_input(0, 0.5);
let output = sim.run_seconds(1.0);

assert!(!output.is_silent());
assert!(output.peak() < 1.0);
```

### RNBO patcher

```rust
use flutmax_sim::RnboSimulator;

let json = std::fs::read_to_string("synth.maxpat").unwrap();
let mut sim = RnboSimulator::from_json_with_sr(&json, 48_000.0).unwrap();

sim.set_param("gain", 0.8);
sim.send_note_on(60, 100); // C4

let output = sim.run_seconds(1.0);
assert!(output.freq_estimate() > 250.0 && output.freq_estimate() < 270.0);
```

## License

MIT
