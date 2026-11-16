//! Deterministic fake [`LiveFrame`] generator for headless scenes and demo mode.

use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::state::LiveFrame;

/// Deterministic generator producing speech-like meters, gain reduction, and 32-band spectrum.
#[derive(Clone, Debug)]
pub struct FakeTelemetry {
    seed: u64,
    sequence: u64,
}

impl Default for FakeTelemetry {
    fn default() -> Self {
        Self::new(0x5449_4E45_0001)
    }
}

impl FakeTelemetry {
    /// Creates a new deterministic generator from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed, sequence: 0 }
    }

    /// Generates a deterministic [`LiveFrame`] at `time_secs` (with `bypass` affecting output).
    pub fn sample(&mut self, time_secs: f32, bypass: bool) -> LiveFrame {
        self.sequence = self.sequence.wrapping_add(1);
        let t = time_secs.max(0.0);

        // Syllabic speech envelope (~3.2 Hz modulation + slower phrasing)
        let phrase = (0.5 + 0.5 * (t * 0.9).sin()).powf(0.6);
        let syllable = 0.5 + 0.5 * (t * 20.0 + 0.7 * (t * 3.1).sin()).sin();
        let jitter = self.next_unit() * 0.08;
        let speech_env = (phrase * (0.35 + 0.65 * syllable) + jitter).clamp(0.02, 1.0);

        let input_rms_db = -42.0 + speech_env * 22.0; // -42 .. -20 dBFS
        let input_peak_db = (input_rms_db + 7.5 + self.next_unit() * 2.5).min(-1.5);

        let (output_rms_db, output_peak_db, gate_gr, comp_gr, deess_gr) = if bypass {
            (input_rms_db, input_peak_db, 0.0, 0.0, 0.0)
        } else {
            let out_rms = (-22.0 + speech_env * 6.0).clamp(-36.0, -14.0);
            let out_peak = (out_rms + 5.5).min(-1.2);
            let gate = if speech_env < 0.22 {
                -18.0 * (1.0 - speech_env / 0.22)
            } else {
                0.0
            };
            let comp = if speech_env > 0.45 {
                -((speech_env - 0.45) * 9.0)
            } else {
                0.0
            };
            let deess = if syllable > 0.78 {
                -((syllable - 0.78) * 14.0)
            } else {
                0.0
            };
            (out_rms, out_peak, gate, comp, deess)
        };

        // 32-band voice-shaped spectrum (fundamental ~120 Hz around band 4, formants around bands 8..18, air 24..31)
        let mut spectrum = [0.0_f32; 32];
        for (i, band) in spectrum.iter_mut().enumerate() {
            let x = i as f32 / 31.0;
            let fundamental = (-((x - 0.14) / 0.09).powi(2)).exp();
            let formant1 = 0.75 * (-((x - 0.34) / 0.14).powi(2)).exp();
            let formant2 = 0.55 * (-((x - 0.58) / 0.16).powi(2)).exp();
            let sibilance = (0.28 + 0.22 * syllable) * (-((x - 0.78) / 0.12).powi(2)).exp();
            let ripple = 0.85 + 0.15 * (t * 7.0 + i as f32 * 0.9).sin();
            *band = (speech_env * (fundamental + formant1 + formant2 + sibilance) * ripple).clamp(0.02, 1.0);
        }

        LiveFrame {
            telemetry: AudioTelemetryFrame {
                sequence: self.sequence,
                flags: u32::from(bypass),
                input_rms_db,
                input_peak_db,
                output_rms_db,
                output_peak_db,
                gate_attenuation_db: gate_gr,
                comp_gain_reduction_db: comp_gr,
                deesser_reduction_db: deess_gr,
            },
            spectrum,
        }
    }

    fn next_unit(&mut self) -> f32 {
        // xorshift64*
        let mut x = self
            .seed
            .wrapping_add(self.sequence.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.seed = x;
        let bits = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as u32;
        (bits as f32) / ((1u32 << 24) as f32)
    }
}

/// Convenience helper returning a deterministic [`LiveFrame`] for static scenes.
#[must_use]
pub fn scene_live_frame(time_secs: f32, bypass: bool) -> LiveFrame {
    let mut tel = FakeTelemetry::default();
    tel.sample(time_secs, bypass)
}
