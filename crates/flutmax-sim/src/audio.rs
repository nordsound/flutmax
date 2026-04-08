/// Audio output buffer with analysis utilities for automated testing.

#[derive(Debug, Clone)]
pub struct AudioOutput {
    pub channels: Vec<Vec<f64>>,
    pub sample_rate: f64,
}

impl AudioOutput {
    /// Create a new AudioOutput with the given number of channels.
    pub fn new(num_channels: usize, sample_rate: f64) -> Self {
        Self {
            channels: vec![Vec::new(); num_channels],
            sample_rate,
        }
    }

    /// Peak absolute amplitude across all channels.
    pub fn peak(&self) -> f64 {
        self.channels
            .iter()
            .flat_map(|ch| ch.iter())
            .fold(0.0_f64, |acc, &x| acc.max(x.abs()))
    }

    /// RMS (root mean square) across all channels combined.
    pub fn rms(&self) -> f64 {
        let total: usize = self.channels.iter().map(|ch| ch.len()).sum();
        if total == 0 {
            return 0.0;
        }
        let sum_sq: f64 = self
            .channels
            .iter()
            .flat_map(|ch| ch.iter())
            .map(|&x| x * x)
            .sum();
        (sum_sq / total as f64).sqrt()
    }

    /// RMS over a sample range of the first channel.
    pub fn rms_range(&self, start: usize, end: usize) -> f64 {
        if self.channels.is_empty() {
            return 0.0;
        }
        let ch = &self.channels[0];
        let end = end.min(ch.len());
        if start >= end {
            return 0.0;
        }
        let sum_sq: f64 = ch[start..end].iter().map(|&x| x * x).sum();
        (sum_sq / (end - start) as f64).sqrt()
    }

    /// Estimate the fundamental frequency using autocorrelation on the first channel.
    ///
    /// Uses the normalized autocorrelation and finds the first peak after
    /// the correlation dips below a threshold, which corresponds to the
    /// fundamental period.
    pub fn freq_estimate(&self) -> f64 {
        if self.channels.is_empty() || self.channels[0].len() < 4 {
            return 0.0;
        }
        let signal = &self.channels[0];
        let len = signal.len();
        // Use up to 4096 samples for analysis
        let n = len.min(4096);
        let signal = &signal[..n];

        let min_lag = (self.sample_rate / 20000.0).ceil() as usize; // Max ~20kHz
        let max_lag = (self.sample_rate / 20.0).ceil() as usize; // Min ~20Hz
        let max_lag = max_lag.min(n / 2);

        if min_lag >= max_lag {
            return 0.0;
        }

        // Compute autocorrelation at lag 0 for normalization
        let energy: f64 = signal.iter().map(|&x| x * x).sum();
        if energy < 1e-12 {
            return 0.0;
        }

        // Compute normalized autocorrelation for each lag
        let mut corrs: Vec<f64> = Vec::with_capacity(max_lag + 1);
        for lag in 0..=max_lag {
            let mut corr = 0.0;
            let count = n - lag;
            for i in 0..count {
                corr += signal[i] * signal[i + lag];
            }
            // Normalize by the geometric mean of the energies of the two segments
            let e1: f64 = signal[..count].iter().map(|&x| x * x).sum();
            let e2: f64 = signal[lag..lag + count].iter().map(|&x| x * x).sum();
            let norm = (e1 * e2).sqrt();
            if norm > 1e-12 {
                corrs.push(corr / norm);
            } else {
                corrs.push(0.0);
            }
        }

        // Find the first peak: look for where correlation dips below a threshold
        // then rises back up. The first peak after the dip is the fundamental.
        let threshold = 0.5;
        let mut found_dip = false;

        for lag in min_lag..max_lag {
            if !found_dip {
                if corrs[lag] < threshold {
                    found_dip = true;
                }
            } else {
                // Look for a peak: corrs[lag] > corrs[lag-1] && corrs[lag] > corrs[lag+1]
                if lag + 1 <= max_lag
                    && corrs[lag] >= corrs[lag - 1]
                    && corrs[lag] >= corrs[lag + 1]
                    && corrs[lag] > 0.0
                {
                    return self.sample_rate / lag as f64;
                }
            }
        }

        0.0
    }

    /// Returns true if the signal is effectively silent (peak < threshold).
    pub fn is_silent(&self) -> bool {
        self.peak() < 1e-6
    }

    /// Returns true if the signal sustains above the given RMS threshold
    /// throughout its duration.
    pub fn is_sustained(&self, threshold: f64) -> bool {
        if self.channels.is_empty() || self.channels[0].is_empty() {
            return false;
        }
        let ch = &self.channels[0];
        let chunk_size = (self.sample_rate * 0.01) as usize; // 10ms chunks
        let chunk_size = chunk_size.max(1);

        for chunk in ch.chunks(chunk_size) {
            let rms: f64 = (chunk.iter().map(|&x| x * x).sum::<f64>() / chunk.len() as f64).sqrt();
            if rms < threshold {
                return false;
            }
        }
        true
    }

    /// Returns true if the signal's RMS is decaying over time.
    pub fn is_decaying(&self) -> bool {
        if self.channels.is_empty() || self.channels[0].len() < 2 {
            return false;
        }
        let ch = &self.channels[0];
        let len = ch.len();
        let quarter = len / 4;
        if quarter == 0 {
            return false;
        }

        let rms_first = (ch[..quarter].iter().map(|&x| x * x).sum::<f64>() / quarter as f64).sqrt();
        let rms_last =
            (ch[len - quarter..].iter().map(|&x| x * x).sum::<f64>() / quarter as f64).sqrt();

        rms_last < rms_first * 0.5
    }

    /// Returns true if the estimated frequency is near the target (within tolerance Hz).
    pub fn freq_near(&self, target: f64, tolerance: f64) -> bool {
        let est = self.freq_estimate();
        (est - target).abs() <= tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine_wave(freq: f64, sample_rate: f64, num_samples: usize, amplitude: f64) -> Vec<f64> {
        (0..num_samples)
            .map(|i| amplitude * (2.0 * PI * freq * i as f64 / sample_rate).sin())
            .collect()
    }

    #[test]
    fn test_peak() {
        let mut output = AudioOutput::new(1, 44100.0);
        output.channels[0] = sine_wave(440.0, 44100.0, 4410, 0.5);
        let peak = output.peak();
        assert!((peak - 0.5).abs() < 0.01, "peak should be ~0.5, got {peak}");
    }

    #[test]
    fn test_rms_sine() {
        let mut output = AudioOutput::new(1, 44100.0);
        // RMS of a sine wave with amplitude A is A / sqrt(2)
        output.channels[0] = sine_wave(440.0, 44100.0, 44100, 1.0);
        let rms = output.rms();
        let expected = 1.0 / 2.0_f64.sqrt();
        assert!(
            (rms - expected).abs() < 0.01,
            "rms should be ~{expected}, got {rms}"
        );
    }

    #[test]
    fn test_is_silent() {
        let output = AudioOutput::new(1, 44100.0);
        assert!(output.is_silent());

        let mut output2 = AudioOutput::new(1, 44100.0);
        output2.channels[0] = sine_wave(440.0, 44100.0, 100, 0.5);
        assert!(!output2.is_silent());
    }

    #[test]
    fn test_freq_estimate() {
        let mut output = AudioOutput::new(1, 44100.0);
        output.channels[0] = sine_wave(440.0, 44100.0, 4096, 1.0);
        let freq = output.freq_estimate();
        assert!(
            (freq - 440.0).abs() < 5.0,
            "freq should be ~440, got {freq}"
        );
    }

    #[test]
    fn test_is_decaying() {
        let mut output = AudioOutput::new(1, 44100.0);
        let n = 44100;
        // Exponentially decaying sine
        output.channels[0] = (0..n)
            .map(|i| {
                let t = i as f64 / 44100.0;
                (-t * 5.0).exp() * (2.0 * PI * 440.0 * t).sin()
            })
            .collect();
        assert!(output.is_decaying());
    }

    #[test]
    fn test_rms_range() {
        let mut output = AudioOutput::new(1, 44100.0);
        output.channels[0] = vec![0.0; 100];
        output.channels[0][50] = 1.0;
        let rms = output.rms_range(40, 60);
        assert!(rms > 0.0);
        let rms_silent = output.rms_range(0, 10);
        assert_eq!(rms_silent, 0.0);
    }

    #[test]
    fn test_freq_near() {
        let mut output = AudioOutput::new(1, 44100.0);
        output.channels[0] = sine_wave(440.0, 44100.0, 4096, 1.0);
        assert!(output.freq_near(440.0, 10.0));
        assert!(!output.freq_near(880.0, 10.0));
    }
}
