//! `flutmax-sim` — gen~ and RNBO DSP simulator.
//!
//! Reads compiled .maxpat JSON and executes the signal graph sample-by-sample,
//! enabling automated audio testing without Max.
//!
//! # Modules
//!
//! - [`ops`] — gen~ operator definitions and execution
//! - [`gen_sim`] — gen~ patcher simulator
//! - [`rnbo_sim`] — RNBO patcher simulator
//! - [`audio`] — Audio output analysis utilities
//! - [`midi`] — MIDI state parser

pub mod audio;
pub mod gen_sim;
pub mod midi;
pub mod ops;
pub mod rnbo_sim;

pub use audio::AudioOutput;
pub use gen_sim::{GenSimulator, SimError};
pub use midi::MidiState;
pub use ops::GenOp;
pub use rnbo_sim::RnboSimulator;
