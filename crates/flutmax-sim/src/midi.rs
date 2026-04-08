/// MIDI state tracker for RNBO simulation.
///
/// Processes raw MIDI bytes and maintains the current state of
/// note, velocity, channel, aftertouch, and CC values.

#[derive(Debug, Clone)]
pub struct MidiState {
    pub note: f64,
    pub velocity: f64,
    pub channel: f64,
    pub aftertouch: f64,
    pub cc: [f64; 128],
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
            note: 0.0,
            velocity: 0.0,
            channel: 1.0,
            aftertouch: 0.0,
            cc: [0.0; 128],
        }
    }
}

impl MidiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process raw MIDI bytes and update internal state.
    ///
    /// Handles: Note On (0x90), Note Off (0x80), Channel Aftertouch (0xD0), CC (0xB0).
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let status = bytes[0] & 0xF0;
        let channel = (bytes[0] & 0x0F) as f64 + 1.0; // MIDI channels are 1-based in Max

        match status {
            // Note Off
            0x80 => {
                if bytes.len() >= 3 {
                    self.note = bytes[1] as f64;
                    self.velocity = 0.0;
                    self.channel = channel;
                }
            }
            // Note On
            0x90 => {
                if bytes.len() >= 3 {
                    self.note = bytes[1] as f64;
                    self.velocity = bytes[2] as f64;
                    self.channel = channel;
                    // Note On with velocity 0 is treated as Note Off
                    if bytes[2] == 0 {
                        self.velocity = 0.0;
                    }
                }
            }
            // Control Change
            0xB0 => {
                if bytes.len() >= 3 {
                    let cc_num = bytes[1] as usize;
                    if cc_num < 128 {
                        self.cc[cc_num] = bytes[2] as f64;
                    }
                    self.channel = channel;
                }
            }
            // Channel Aftertouch
            0xD0 => {
                if bytes.len() >= 2 {
                    self.aftertouch = bytes[1] as f64;
                    self.channel = channel;
                }
            }
            _ => {} // Ignore other messages
        }
    }

    /// Convenience: send a Note On message.
    pub fn note_on(&mut self, note: u8, velocity: u8) {
        self.process_bytes(&[0x90, note, velocity]);
    }

    /// Convenience: send a Note Off message.
    pub fn note_off(&mut self, note: u8) {
        self.process_bytes(&[0x80, note, 0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_on() {
        let mut state = MidiState::new();
        state.process_bytes(&[0x90, 60, 100]);
        assert_eq!(state.note, 60.0);
        assert_eq!(state.velocity, 100.0);
        assert_eq!(state.channel, 1.0);
    }

    #[test]
    fn test_note_off() {
        let mut state = MidiState::new();
        state.process_bytes(&[0x90, 60, 100]);
        state.process_bytes(&[0x80, 60, 0]);
        assert_eq!(state.note, 60.0);
        assert_eq!(state.velocity, 0.0);
    }

    #[test]
    fn test_note_on_velocity_zero_is_note_off() {
        let mut state = MidiState::new();
        state.process_bytes(&[0x90, 60, 0]);
        assert_eq!(state.velocity, 0.0);
    }

    #[test]
    fn test_channel() {
        let mut state = MidiState::new();
        // Channel 10 (0-indexed 9)
        state.process_bytes(&[0x99, 36, 127]);
        assert_eq!(state.channel, 10.0);
    }

    #[test]
    fn test_cc() {
        let mut state = MidiState::new();
        state.process_bytes(&[0xB0, 1, 64]); // CC#1 = mod wheel
        assert_eq!(state.cc[1], 64.0);
    }

    #[test]
    fn test_aftertouch() {
        let mut state = MidiState::new();
        state.process_bytes(&[0xD0, 80]);
        assert_eq!(state.aftertouch, 80.0);
    }

    #[test]
    fn test_convenience_methods() {
        let mut state = MidiState::new();
        state.note_on(72, 110);
        assert_eq!(state.note, 72.0);
        assert_eq!(state.velocity, 110.0);
        state.note_off(72);
        assert_eq!(state.velocity, 0.0);
    }

    #[test]
    fn test_empty_bytes() {
        let mut state = MidiState::new();
        state.process_bytes(&[]);
        assert_eq!(state.note, 0.0);
    }
}
