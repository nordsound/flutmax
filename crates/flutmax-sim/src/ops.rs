/// gen~ operator definitions and execution logic.
use std::f64::consts::PI;

/// All supported gen~ operators.
#[derive(Debug, Clone, PartialEq)]
pub enum GenOp {
    // I/O
    In(usize),
    Out(usize),

    // Arithmetic (binary with optional literal arg)
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // Unary
    Neg,
    Abs,
    Sign,

    // Comparison
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    Neq,
    Max,
    Min,

    // Math
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Exp2,
    Log,
    Log2,
    Sqrt,
    Pow,
    Tanh,
    Sinh,
    Cosh,

    // Rounding
    Ceil,
    Floor,
    Round,
    Trunc,
    Fract,

    // Clamping
    Clip,
    Wrap,
    Fold,
    Clamp,

    // Conversion
    Mtof,
    Ftom,
    Dbtoa,
    Atodb,
    Mstosamps,
    Sampstoms,

    // State
    History,
    Delay,

    // Generators
    Noise,
    SampleRate,
    Cycle,
    Phasor,

    // Logic
    And,
    Or,
    Xor,
    Not,
    Switch,
    Gate,
    Selector,

    // Utility
    Fixdenorm,
    Change,
    Delta,
    Latch,
    Accum,
    Counter,

    // Param (for gen~ @param)
    Param,

    // Pass-through for unknown ops
    Pass,
}

/// Ring buffer for delay lines.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    data: Vec<f64>,
    write_pos: usize,
}

impl RingBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            data: vec![0.0; max_size.max(1)],
            write_pos: 0,
        }
    }

    pub fn write(&mut self, value: f64) {
        self.data[self.write_pos] = value;
        self.write_pos = (self.write_pos + 1) % self.data.len();
    }

    pub fn read(&self, delay: usize) -> f64 {
        if delay == 0 || self.data.is_empty() {
            return 0.0;
        }
        let delay = delay.min(self.data.len());
        let pos = (self.write_pos + self.data.len() - delay) % self.data.len();
        self.data[pos]
    }
}

/// Per-node state for stateful operators.
#[derive(Debug, Clone)]
pub enum NodeState {
    None,
    History(f64),
    Delay(RingBuffer),
    Phasor(f64),
    CyclePhasor(f64),
    /// Change: stores previous value
    Change(f64),
    /// Delta: stores previous value
    Delta(f64),
    /// Latch: stores latched value
    Latch(f64),
    /// Accum: stores accumulated value
    Accum(f64),
    /// Counter: stores count
    Counter(f64),
}

/// Sine wavetable for cycle~ (512 points).
const WAVETABLE_SIZE: usize = 512;

fn wavetable_lookup(phase: f64) -> f64 {
    // phase is 0..1, wrap it
    let phase = phase.fract();
    let phase = if phase < 0.0 { phase + 1.0 } else { phase };
    let pos = phase * WAVETABLE_SIZE as f64;
    let idx0 = pos as usize % WAVETABLE_SIZE;
    let idx1 = (idx0 + 1) % WAVETABLE_SIZE;
    let frac = pos - pos.floor();

    let v0 = (2.0 * PI * idx0 as f64 / WAVETABLE_SIZE as f64).sin();
    let v1 = (2.0 * PI * idx1 as f64 / WAVETABLE_SIZE as f64).sin();

    v0 + frac * (v1 - v0) // linear interpolation
}

/// MIDI note to frequency conversion (A4 = 440 Hz, note 69).
fn mtof(note: f64) -> f64 {
    440.0 * 2.0_f64.powf((note - 69.0) / 12.0)
}

/// Frequency to MIDI note.
fn ftom(freq: f64) -> f64 {
    if freq <= 0.0 {
        return 0.0;
    }
    69.0 + 12.0 * (freq / 440.0).log2()
}

/// dB to amplitude.
fn dbtoa(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

/// Amplitude to dB.
fn atodb(amp: f64) -> f64 {
    if amp <= 0.0 {
        return -999.0;
    }
    20.0 * amp.log10()
}

/// Fix denormalized floats (replace subnormal with 0).
fn fixdenorm(x: f64) -> f64 {
    if x.is_subnormal() || x.is_nan() {
        0.0
    } else {
        x
    }
}

/// Wrap a value into the range [min, max).
fn wrap_range(x: f64, lo: f64, hi: f64) -> f64 {
    if hi <= lo {
        return lo;
    }
    let range = hi - lo;
    let mut v = (x - lo) % range;
    if v < 0.0 {
        v += range;
    }
    v + lo
}

/// Fold a value into the range [min, max] by reflecting at boundaries.
fn fold_range(x: f64, lo: f64, hi: f64) -> f64 {
    if hi <= lo {
        return lo;
    }
    let range = hi - lo;
    let mut v = x - lo;
    // Normalize to [0, range*2)
    let period = range * 2.0;
    v = v % period;
    if v < 0.0 {
        v += period;
    }
    // Fold
    if v > range {
        v = period - v;
    }
    v + lo
}

/// Execute a gen~ operator for one sample.
///
/// Returns a vector of output values (one per outlet).
pub fn execute_op(
    op: &GenOp,
    inputs: &[f64],
    arg: Option<f64>,
    state: &mut NodeState,
    sample_rate: f64,
) -> Vec<f64> {
    let in0 = || inputs.first().copied().unwrap_or(0.0);
    let in1 = || arg.or_else(|| inputs.get(1).copied()).unwrap_or(0.0);
    // For binary ops: if inlet 1 is connected, use it; otherwise use arg; otherwise use default.
    let in1_default = |default: f64| inputs.get(1).copied().or(arg).unwrap_or(default);

    match op {
        // I/O — handled externally
        GenOp::In(_) => vec![in0()],
        GenOp::Out(_) => vec![in0()],
        GenOp::Param => vec![in0()],

        // Arithmetic
        GenOp::Add => vec![in0() + in1()],
        GenOp::Sub => vec![in0() - in1()],
        GenOp::Mul => vec![in0() * in1_default(1.0)],
        GenOp::Div => {
            let b = in1_default(1.0);
            vec![if b == 0.0 { 0.0 } else { in0() / b }]
        }
        GenOp::Mod => {
            let b = in1_default(1.0);
            vec![if b == 0.0 { 0.0 } else { in0() % b }]
        }

        // Unary
        GenOp::Neg => vec![-in0()],
        GenOp::Abs => vec![in0().abs()],
        GenOp::Sign => {
            let x = in0();
            vec![if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }]
        }

        // Comparison
        GenOp::Gt => vec![if in0() > in1() { 1.0 } else { 0.0 }],
        GenOp::Gte => vec![if in0() >= in1() { 1.0 } else { 0.0 }],
        GenOp::Lt => vec![if in0() < in1() { 1.0 } else { 0.0 }],
        GenOp::Lte => vec![if in0() <= in1() { 1.0 } else { 0.0 }],
        GenOp::Eq => vec![if (in0() - in1()).abs() < f64::EPSILON {
            1.0
        } else {
            0.0
        }],
        GenOp::Neq => vec![if (in0() - in1()).abs() >= f64::EPSILON {
            1.0
        } else {
            0.0
        }],
        GenOp::Max => vec![in0().max(in1())],
        GenOp::Min => vec![in0().min(in1())],

        // Math
        GenOp::Sin => vec![in0().sin()],
        GenOp::Cos => vec![in0().cos()],
        GenOp::Tan => vec![in0().tan()],
        GenOp::Asin => vec![in0().asin()],
        GenOp::Acos => vec![in0().acos()],
        GenOp::Atan => vec![in0().atan()],
        GenOp::Atan2 => vec![in0().atan2(in1())],
        GenOp::Exp => vec![in0().exp()],
        GenOp::Exp2 => vec![2.0_f64.powf(in0())],
        GenOp::Log => {
            let x = in0();
            vec![if x > 0.0 { x.ln() } else { -999.0 }]
        }
        GenOp::Log2 => {
            let x = in0();
            vec![if x > 0.0 { x.log2() } else { -999.0 }]
        }
        GenOp::Sqrt => vec![in0().max(0.0).sqrt()],
        GenOp::Pow => vec![in0().powf(in1_default(1.0))],
        GenOp::Tanh => vec![in0().tanh()],
        GenOp::Sinh => vec![in0().sinh()],
        GenOp::Cosh => vec![in0().cosh()],

        // Rounding
        GenOp::Ceil => vec![in0().ceil()],
        GenOp::Floor => vec![in0().floor()],
        GenOp::Round => vec![in0().round()],
        GenOp::Trunc => vec![in0().trunc()],
        GenOp::Fract => vec![in0().fract()],

        // Clamping
        GenOp::Clip | GenOp::Clamp => {
            let x = in0();
            let lo = inputs.get(1).copied().or(arg).unwrap_or(0.0);
            let hi = inputs.get(2).copied().unwrap_or(1.0);
            vec![x.max(lo).min(hi)]
        }
        GenOp::Wrap => {
            let x = in0();
            let lo = inputs.get(1).copied().or(arg).unwrap_or(0.0);
            let hi = inputs.get(2).copied().unwrap_or(1.0);
            vec![wrap_range(x, lo, hi)]
        }
        GenOp::Fold => {
            let x = in0();
            let lo = inputs.get(1).copied().or(arg).unwrap_or(0.0);
            let hi = inputs.get(2).copied().unwrap_or(1.0);
            vec![fold_range(x, lo, hi)]
        }

        // Conversion
        GenOp::Mtof => vec![mtof(in0())],
        GenOp::Ftom => vec![ftom(in0())],
        GenOp::Dbtoa => vec![dbtoa(in0())],
        GenOp::Atodb => vec![atodb(in0())],
        GenOp::Mstosamps => vec![in0() * sample_rate / 1000.0],
        GenOp::Sampstoms => vec![in0() * 1000.0 / sample_rate],

        // State
        GenOp::History => {
            if let NodeState::History(val) = state {
                vec![*val]
            } else {
                vec![0.0]
            }
        }
        GenOp::Delay => {
            if let NodeState::Delay(buf) = state {
                let delay_samples = inputs.get(1).copied().or(arg).unwrap_or(1.0) as usize;
                vec![buf.read(delay_samples)]
            } else {
                vec![0.0]
            }
        }

        // Generators
        GenOp::Noise => {
            use rand::Rng;
            let mut rng = rand::thread_rng();
            vec![rng.gen_range(-1.0..1.0)]
        }
        GenOp::SampleRate => vec![sample_rate],
        GenOp::Cycle => {
            if let NodeState::CyclePhasor(phase) = state {
                let freq = in0();
                let output = wavetable_lookup(*phase);
                *phase += freq / sample_rate;
                if *phase >= 1.0 {
                    *phase -= 1.0;
                }
                if *phase < 0.0 {
                    *phase += 1.0;
                }
                // cycle~ has two outlets: signal and sync (phase)
                vec![output, *phase]
            } else {
                vec![0.0, 0.0]
            }
        }
        GenOp::Phasor => {
            if let NodeState::Phasor(phase) = state {
                let freq = in0();
                let output = *phase;
                *phase += freq / sample_rate;
                if *phase >= 1.0 {
                    *phase -= 1.0;
                }
                if *phase < 0.0 {
                    *phase += 1.0;
                }
                vec![output]
            } else {
                vec![0.0]
            }
        }

        // Logic
        GenOp::And => {
            vec![if in0() != 0.0 && in1() != 0.0 {
                1.0
            } else {
                0.0
            }]
        }
        GenOp::Or => {
            vec![if in0() != 0.0 || in1() != 0.0 {
                1.0
            } else {
                0.0
            }]
        }
        GenOp::Xor => {
            vec![if (in0() != 0.0) ^ (in1() != 0.0) {
                1.0
            } else {
                0.0
            }]
        }
        GenOp::Not => vec![if in0() == 0.0 { 1.0 } else { 0.0 }],
        GenOp::Switch => {
            // switch(sel, a, b): if sel != 0 then a else b
            let sel = in0();
            let a = inputs.get(1).copied().unwrap_or(0.0);
            let b = inputs.get(2).copied().unwrap_or(0.0);
            vec![if sel != 0.0 { a } else { b }]
        }
        GenOp::Gate => {
            // gate N index input: routes input to one of N outlets
            // Simplified: gate with arg = number of outlets
            let index = in0() as usize;
            let input = inputs.get(1).copied().unwrap_or(0.0);
            let n = arg.unwrap_or(1.0) as usize;
            let mut outs = vec![0.0; n.max(1)];
            if index > 0 && index <= outs.len() {
                outs[index - 1] = input;
            }
            outs
        }
        GenOp::Selector => {
            // selector N index in1 in2 ...: selects one of N inputs
            let index = in0() as usize;
            if index > 0 && index < inputs.len() {
                vec![inputs[index]]
            } else {
                vec![0.0]
            }
        }

        // Utility
        GenOp::Fixdenorm => vec![fixdenorm(in0())],
        GenOp::Change => {
            if let NodeState::Change(prev) = state {
                let x = in0();
                let out = if x > *prev {
                    1.0
                } else if x < *prev {
                    -1.0
                } else {
                    0.0
                };
                *prev = x;
                vec![out]
            } else {
                vec![0.0]
            }
        }
        GenOp::Delta => {
            if let NodeState::Delta(prev) = state {
                let x = in0();
                let out = x - *prev;
                *prev = x;
                vec![out]
            } else {
                vec![0.0]
            }
        }
        GenOp::Latch => {
            if let NodeState::Latch(stored) = state {
                let input = in0();
                let trigger = inputs.get(1).copied().unwrap_or(0.0);
                if trigger != 0.0 {
                    *stored = input;
                }
                vec![*stored]
            } else {
                vec![0.0]
            }
        }
        GenOp::Accum => {
            if let NodeState::Accum(acc) = state {
                let input = in0();
                let reset = inputs.get(1).copied().unwrap_or(0.0);
                if reset != 0.0 {
                    *acc = 0.0;
                }
                *acc += input;
                vec![*acc]
            } else {
                vec![0.0]
            }
        }
        GenOp::Counter => {
            if let NodeState::Counter(count) = state {
                let trigger = in0();
                let reset = inputs.get(1).copied().unwrap_or(0.0);
                if reset != 0.0 {
                    *count = 0.0;
                }
                if trigger != 0.0 {
                    *count += 1.0;
                }
                vec![*count]
            } else {
                vec![0.0]
            }
        }

        GenOp::Pass => vec![in0()],
    }
}

/// Parse a gen~ box text field into a (GenOp, optional arg) pair.
pub fn parse_gen_op(text: &str) -> (GenOp, Option<f64>) {
    let text = text.trim();
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.is_empty() {
        return (GenOp::Pass, None);
    }
    let name = parts[0];

    // Handle "in N" and "out N" specially
    if name == "in" {
        let n: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        return (GenOp::In(n - 1), None);
    }
    if name == "out" {
        let n: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
        return (GenOp::Out(n - 1), None);
    }

    // Handle "history [name] [initial_value]"
    if name == "history" {
        // Parse: "history", "history 0", "history fb 0", "history fb"
        let initial = parts
            .iter()
            .rev()
            .find_map(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        return (GenOp::History, Some(initial));
    }

    // Handle "delay [max_samples]"
    if name == "delay" {
        let max_size = parts.get(1).and_then(|s| s.parse::<f64>().ok());
        return (GenOp::Delay, max_size);
    }

    // Handle "param name default min max"
    if name == "param" {
        // Try to find a numeric default value
        let default_val = parts
            .iter()
            .skip(1)
            .find_map(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        return (GenOp::Param, Some(default_val));
    }

    // Handle "gate N"
    if name == "gate" {
        let n = parts.get(1).and_then(|s| s.parse::<f64>().ok());
        return (GenOp::Gate, n);
    }

    // Handle "selector N"
    if name == "selector" {
        let n = parts.get(1).and_then(|s| s.parse::<f64>().ok());
        return (GenOp::Selector, n);
    }

    // For all others, the optional arg is the first numeric token after the name
    let arg = parts.get(1).and_then(|s| s.parse::<f64>().ok());

    let op = match name {
        "+" => GenOp::Add,
        "-" => GenOp::Sub,
        "*" => GenOp::Mul,
        "/" | "div" => GenOp::Div,
        "%" | "mod" => GenOp::Mod,
        "neg" => GenOp::Neg,
        "abs" => GenOp::Abs,
        "sign" => GenOp::Sign,
        ">" | "gt" => GenOp::Gt,
        ">=" | "gte" => GenOp::Gte,
        "<" | "lt" => GenOp::Lt,
        "<=" | "lte" => GenOp::Lte,
        "==" | "eq" => GenOp::Eq,
        "!=" | "neq" => GenOp::Neq,
        "max" | "maximum" => GenOp::Max,
        "min" | "minimum" => GenOp::Min,
        "sin" => GenOp::Sin,
        "cos" => GenOp::Cos,
        "tan" => GenOp::Tan,
        "asin" => GenOp::Asin,
        "acos" => GenOp::Acos,
        "atan" => GenOp::Atan,
        "atan2" => GenOp::Atan2,
        "exp" => GenOp::Exp,
        "exp2" => GenOp::Exp2,
        "log" => GenOp::Log,
        "log2" => GenOp::Log2,
        "sqrt" => GenOp::Sqrt,
        "pow" => GenOp::Pow,
        "tanh" => GenOp::Tanh,
        "sinh" => GenOp::Sinh,
        "cosh" => GenOp::Cosh,
        "ceil" => GenOp::Ceil,
        "floor" => GenOp::Floor,
        "round" => GenOp::Round,
        "trunc" => GenOp::Trunc,
        "fract" => GenOp::Fract,
        "clip" => GenOp::Clip,
        "wrap" => GenOp::Wrap,
        "fold" => GenOp::Fold,
        "clamp" => GenOp::Clamp,
        "mtof" => GenOp::Mtof,
        "ftom" => GenOp::Ftom,
        "dbtoa" => GenOp::Dbtoa,
        "atodb" => GenOp::Atodb,
        "mstosamps" => GenOp::Mstosamps,
        "sampstoms" => GenOp::Sampstoms,
        "noise" => GenOp::Noise,
        "samplerate" => GenOp::SampleRate,
        "cycle~" | "cycle" => GenOp::Cycle,
        "phasor~" | "phasor" => GenOp::Phasor,
        "and" | "&&" => GenOp::And,
        "or" | "||" => GenOp::Or,
        "xor" => GenOp::Xor,
        "not" | "!" => GenOp::Not,
        "switch" => GenOp::Switch,
        "fixdenorm" => GenOp::Fixdenorm,
        "change" => GenOp::Change,
        "delta" => GenOp::Delta,
        "latch" => GenOp::Latch,
        "accum" => GenOp::Accum,
        "counter" => GenOp::Counter,
        _ => GenOp::Pass,
    };
    (op, arg)
}

/// Determine number of outlets for a given op.
pub fn num_outlets(op: &GenOp) -> usize {
    match op {
        GenOp::Cycle => 2, // signal + sync
        GenOp::Gate => 1,  // actual count comes from arg, but we handle dynamically
        _ => 1,
    }
}

/// Create the appropriate initial NodeState for an operator.
pub fn initial_state(op: &GenOp, arg: Option<f64>) -> NodeState {
    match op {
        GenOp::History => NodeState::History(arg.unwrap_or(0.0)),
        GenOp::Delay => {
            let max_size = arg.unwrap_or(48000.0) as usize;
            NodeState::Delay(RingBuffer::new(max_size.max(1)))
        }
        GenOp::Phasor => NodeState::Phasor(0.0),
        GenOp::Cycle => NodeState::CyclePhasor(0.0),
        GenOp::Change => NodeState::Change(0.0),
        GenOp::Delta => NodeState::Delta(0.0),
        GenOp::Latch => NodeState::Latch(0.0),
        GenOp::Accum => NodeState::Accum(0.0),
        GenOp::Counter => NodeState::Counter(0.0),
        _ => NodeState::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(op: GenOp, inputs: &[f64], arg: Option<f64>) -> Vec<f64> {
        let mut state = initial_state(&op, arg);
        execute_op(&op, inputs, arg, &mut state, 44100.0)
    }

    // Arithmetic
    #[test]
    fn test_add() {
        assert_eq!(exec(GenOp::Add, &[3.0, 4.0], None), vec![7.0]);
    }

    #[test]
    fn test_add_with_arg() {
        assert_eq!(exec(GenOp::Add, &[3.0], Some(0.5)), vec![3.5]);
    }

    #[test]
    fn test_sub() {
        assert_eq!(exec(GenOp::Sub, &[10.0, 3.0], None), vec![7.0]);
    }

    #[test]
    fn test_mul() {
        assert_eq!(exec(GenOp::Mul, &[3.0, 4.0], None), vec![12.0]);
    }

    #[test]
    fn test_mul_default() {
        // No input 1 and no arg → default 1.0
        assert_eq!(exec(GenOp::Mul, &[5.0], None), vec![5.0]);
    }

    #[test]
    fn test_div() {
        assert_eq!(exec(GenOp::Div, &[10.0, 2.0], None), vec![5.0]);
    }

    #[test]
    fn test_div_by_zero() {
        assert_eq!(exec(GenOp::Div, &[10.0, 0.0], None), vec![0.0]);
    }

    #[test]
    fn test_mod() {
        assert_eq!(exec(GenOp::Mod, &[7.0, 3.0], None), vec![1.0]);
    }

    // Unary
    #[test]
    fn test_neg() {
        assert_eq!(exec(GenOp::Neg, &[5.0], None), vec![-5.0]);
    }

    #[test]
    fn test_abs() {
        assert_eq!(exec(GenOp::Abs, &[-3.0], None), vec![3.0]);
    }

    #[test]
    fn test_sign() {
        assert_eq!(exec(GenOp::Sign, &[5.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Sign, &[-5.0], None), vec![-1.0]);
        assert_eq!(exec(GenOp::Sign, &[0.0], None), vec![0.0]);
    }

    // Comparison
    #[test]
    fn test_gt() {
        assert_eq!(exec(GenOp::Gt, &[5.0, 3.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Gt, &[3.0, 5.0], None), vec![0.0]);
    }

    #[test]
    fn test_eq() {
        assert_eq!(exec(GenOp::Eq, &[3.0, 3.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Eq, &[3.0, 4.0], None), vec![0.0]);
    }

    #[test]
    fn test_max_min() {
        assert_eq!(exec(GenOp::Max, &[3.0, 5.0], None), vec![5.0]);
        assert_eq!(exec(GenOp::Min, &[3.0, 5.0], None), vec![3.0]);
    }

    // Math
    #[test]
    fn test_sin() {
        let result = exec(GenOp::Sin, &[PI / 2.0], None);
        assert!((result[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_cos() {
        let result = exec(GenOp::Cos, &[0.0], None);
        assert!((result[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_exp() {
        let result = exec(GenOp::Exp, &[1.0], None);
        assert!((result[0] - std::f64::consts::E).abs() < 1e-10);
    }

    #[test]
    fn test_sqrt() {
        assert_eq!(exec(GenOp::Sqrt, &[9.0], None), vec![3.0]);
    }

    #[test]
    fn test_pow() {
        assert_eq!(exec(GenOp::Pow, &[2.0, 3.0], None), vec![8.0]);
    }

    #[test]
    fn test_tanh() {
        let result = exec(GenOp::Tanh, &[0.0], None);
        assert!((result[0]).abs() < 1e-10);
    }

    // Rounding
    #[test]
    fn test_ceil() {
        assert_eq!(exec(GenOp::Ceil, &[3.2], None), vec![4.0]);
    }

    #[test]
    fn test_floor() {
        assert_eq!(exec(GenOp::Floor, &[3.7], None), vec![3.0]);
    }

    #[test]
    fn test_round() {
        assert_eq!(exec(GenOp::Round, &[3.5], None), vec![4.0]);
        assert_eq!(exec(GenOp::Round, &[3.4], None), vec![3.0]);
    }

    #[test]
    fn test_trunc() {
        assert_eq!(exec(GenOp::Trunc, &[-3.7], None), vec![-3.0]);
    }

    #[test]
    fn test_fract() {
        let result = exec(GenOp::Fract, &[3.75], None);
        assert!((result[0] - 0.75).abs() < 1e-10);
    }

    // Clamping
    #[test]
    fn test_clip() {
        assert_eq!(exec(GenOp::Clip, &[5.0, 0.0, 1.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Clip, &[-0.5, 0.0, 1.0], None), vec![0.0]);
        assert_eq!(exec(GenOp::Clip, &[0.5, 0.0, 1.0], None), vec![0.5]);
    }

    #[test]
    fn test_wrap() {
        let result = exec(GenOp::Wrap, &[1.5, 0.0, 1.0], None);
        assert!((result[0] - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_fold() {
        let result = exec(GenOp::Fold, &[1.3, 0.0, 1.0], None);
        assert!((result[0] - 0.7).abs() < 1e-10);
    }

    // Conversion
    #[test]
    fn test_mtof() {
        let result = exec(GenOp::Mtof, &[69.0], None);
        assert!((result[0] - 440.0).abs() < 0.01);
    }

    #[test]
    fn test_ftom() {
        let result = exec(GenOp::Ftom, &[440.0], None);
        assert!((result[0] - 69.0).abs() < 0.01);
    }

    #[test]
    fn test_dbtoa() {
        let result = exec(GenOp::Dbtoa, &[0.0], None);
        assert!((result[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_atodb() {
        let result = exec(GenOp::Atodb, &[1.0], None);
        assert!((result[0]).abs() < 1e-10);
    }

    #[test]
    fn test_mstosamps() {
        // 1ms at 44100 = 44.1 samples
        let result = exec(GenOp::Mstosamps, &[1.0], None);
        assert!((result[0] - 44.1).abs() < 0.01);
    }

    #[test]
    fn test_sampstoms() {
        let result = exec(GenOp::Sampstoms, &[44100.0], None);
        assert!((result[0] - 1000.0).abs() < 0.01);
    }

    // Generators
    #[test]
    fn test_samplerate() {
        assert_eq!(exec(GenOp::SampleRate, &[], None), vec![44100.0]);
    }

    #[test]
    fn test_noise() {
        let result = exec(GenOp::Noise, &[], None);
        assert!(result[0] >= -1.0 && result[0] <= 1.0);
    }

    #[test]
    fn test_phasor() {
        let mut state = NodeState::Phasor(0.0);
        // 1 Hz at 44100 SR: phase increments by 1/44100 each sample
        let result = execute_op(&GenOp::Phasor, &[1.0], None, &mut state, 44100.0);
        assert!((result[0]).abs() < 1e-10); // First output is 0
        let result = execute_op(&GenOp::Phasor, &[1.0], None, &mut state, 44100.0);
        assert!((result[0] - 1.0 / 44100.0).abs() < 1e-10);
    }

    #[test]
    fn test_cycle() {
        let mut state = NodeState::CyclePhasor(0.0);
        // At phase 0, cycle~ should output sin(0) = 0
        let result = execute_op(&GenOp::Cycle, &[440.0], None, &mut state, 44100.0);
        assert!(result[0].abs() < 0.02); // Near zero at phase 0
        assert_eq!(result.len(), 2); // signal + sync
    }

    // Logic
    #[test]
    fn test_and() {
        assert_eq!(exec(GenOp::And, &[1.0, 1.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::And, &[1.0, 0.0], None), vec![0.0]);
    }

    #[test]
    fn test_or() {
        assert_eq!(exec(GenOp::Or, &[0.0, 1.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Or, &[0.0, 0.0], None), vec![0.0]);
    }

    #[test]
    fn test_not() {
        assert_eq!(exec(GenOp::Not, &[0.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Not, &[1.0], None), vec![0.0]);
    }

    #[test]
    fn test_switch() {
        assert_eq!(exec(GenOp::Switch, &[1.0, 10.0, 20.0], None), vec![10.0]);
        assert_eq!(exec(GenOp::Switch, &[0.0, 10.0, 20.0], None), vec![20.0]);
    }

    // Utility
    #[test]
    fn test_change() {
        let mut state = NodeState::Change(0.0);
        let r1 = execute_op(&GenOp::Change, &[1.0], None, &mut state, 44100.0);
        assert_eq!(r1, vec![1.0]); // increased
        let r2 = execute_op(&GenOp::Change, &[1.0], None, &mut state, 44100.0);
        assert_eq!(r2, vec![0.0]); // same
        let r3 = execute_op(&GenOp::Change, &[0.5], None, &mut state, 44100.0);
        assert_eq!(r3, vec![-1.0]); // decreased
    }

    #[test]
    fn test_delta() {
        let mut state = NodeState::Delta(0.0);
        let r1 = execute_op(&GenOp::Delta, &[5.0], None, &mut state, 44100.0);
        assert_eq!(r1, vec![5.0]);
        let r2 = execute_op(&GenOp::Delta, &[8.0], None, &mut state, 44100.0);
        assert_eq!(r2, vec![3.0]);
    }

    #[test]
    fn test_latch() {
        let mut state = NodeState::Latch(0.0);
        // No trigger
        let r1 = execute_op(&GenOp::Latch, &[5.0, 0.0], None, &mut state, 44100.0);
        assert_eq!(r1, vec![0.0]); // Still 0
                                   // Trigger
        let r2 = execute_op(&GenOp::Latch, &[5.0, 1.0], None, &mut state, 44100.0);
        assert_eq!(r2, vec![5.0]); // Latched
                                   // Holds
        let r3 = execute_op(&GenOp::Latch, &[10.0, 0.0], None, &mut state, 44100.0);
        assert_eq!(r3, vec![5.0]); // Still latched
    }

    #[test]
    fn test_accum() {
        let mut state = NodeState::Accum(0.0);
        execute_op(&GenOp::Accum, &[1.0, 0.0], None, &mut state, 44100.0);
        execute_op(&GenOp::Accum, &[1.0, 0.0], None, &mut state, 44100.0);
        let r = execute_op(&GenOp::Accum, &[1.0, 0.0], None, &mut state, 44100.0);
        assert_eq!(r, vec![3.0]);
    }

    #[test]
    fn test_fixdenorm() {
        assert_eq!(exec(GenOp::Fixdenorm, &[1.0], None), vec![1.0]);
        assert_eq!(exec(GenOp::Fixdenorm, &[0.0], None), vec![0.0]);
    }

    // History
    #[test]
    fn test_history() {
        let mut state = NodeState::History(0.5);
        let result = execute_op(&GenOp::History, &[], None, &mut state, 44100.0);
        assert_eq!(result, vec![0.5]);
    }

    // Delay
    #[test]
    fn test_delay() {
        let mut state = NodeState::Delay(RingBuffer::new(100));
        // Write some values
        if let NodeState::Delay(buf) = &mut state {
            buf.write(1.0);
            buf.write(2.0);
            buf.write(3.0);
        }
        // Read with delay of 1 (most recent)
        let r = execute_op(&GenOp::Delay, &[0.0, 1.0], None, &mut state, 44100.0);
        assert_eq!(r, vec![3.0]);
        // Read with delay of 3 (oldest)
        let r = execute_op(&GenOp::Delay, &[0.0, 3.0], None, &mut state, 44100.0);
        assert_eq!(r, vec![1.0]);
    }

    // Ring buffer
    #[test]
    fn test_ring_buffer() {
        let mut buf = RingBuffer::new(4);
        buf.write(10.0);
        buf.write(20.0);
        buf.write(30.0);
        assert_eq!(buf.read(1), 30.0);
        assert_eq!(buf.read(2), 20.0);
        assert_eq!(buf.read(3), 10.0);
    }

    #[test]
    fn test_ring_buffer_wrap() {
        let mut buf = RingBuffer::new(3);
        buf.write(1.0);
        buf.write(2.0);
        buf.write(3.0);
        buf.write(4.0); // Overwrites 1.0
        assert_eq!(buf.read(1), 4.0);
        assert_eq!(buf.read(2), 3.0);
        assert_eq!(buf.read(3), 2.0);
    }

    // parse_gen_op
    #[test]
    fn test_parse_add() {
        let (op, arg) = parse_gen_op("+ 0.5");
        assert_eq!(op, GenOp::Add);
        assert_eq!(arg, Some(0.5));
    }

    #[test]
    fn test_parse_mul() {
        let (op, arg) = parse_gen_op("* 0.3");
        assert_eq!(op, GenOp::Mul);
        assert_eq!(arg, Some(0.3));
    }

    #[test]
    fn test_parse_in() {
        let (op, _) = parse_gen_op("in 3");
        assert_eq!(op, GenOp::In(2)); // 1-based → 0-based
    }

    #[test]
    fn test_parse_out() {
        let (op, _) = parse_gen_op("out 1");
        assert_eq!(op, GenOp::Out(0));
    }

    #[test]
    fn test_parse_history() {
        let (op, arg) = parse_gen_op("history fb 0");
        assert_eq!(op, GenOp::History);
        assert_eq!(arg, Some(0.0));
    }

    #[test]
    fn test_parse_history_unnamed() {
        let (op, arg) = parse_gen_op("history 0.5");
        assert_eq!(op, GenOp::History);
        assert_eq!(arg, Some(0.5));
    }

    #[test]
    fn test_parse_neg() {
        let (op, arg) = parse_gen_op("neg");
        assert_eq!(op, GenOp::Neg);
        assert_eq!(arg, None);
    }

    #[test]
    fn test_parse_delay() {
        let (op, arg) = parse_gen_op("delay");
        assert_eq!(op, GenOp::Delay);
        assert_eq!(arg, None);
    }

    #[test]
    fn test_parse_cycle() {
        let (op, _) = parse_gen_op("cycle~");
        assert_eq!(op, GenOp::Cycle);
    }

    #[test]
    fn test_parse_div() {
        let (op, arg) = parse_gen_op("div 2");
        assert_eq!(op, GenOp::Div);
        assert_eq!(arg, Some(2.0));
    }

    #[test]
    fn test_parse_samplerate() {
        let (op, _) = parse_gen_op("samplerate");
        assert_eq!(op, GenOp::SampleRate);
    }

    #[test]
    fn test_parse_unknown() {
        let (op, _) = parse_gen_op("unknown_thing");
        assert_eq!(op, GenOp::Pass);
    }
}
