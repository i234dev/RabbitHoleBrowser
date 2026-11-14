//! Private, device-independent dynamics compressor. One instance belongs to
//! one graph node; input frames must already contain the sum of its inputs.
//!
//! Primary contract: W3C Web Audio Recommendation (2021), section 1.19.4:
//! https://www.w3.org/TR/2021/REC-webaudio-20210617/#dynamicscompressornode-processing
//! See docs/webaudio-compressor-core.md for the chosen curves and limitations.

/// The renderer samples the five parameters once per quantum. This core does
/// not schedule automation or read a JS object from an audio device thread.
pub(super) const QUANTUM: usize = 128;
const LOOK_AHEAD_SECONDS: f64 = 0.006;

/// Internal frames are always canonical stereo, with a shared detector/gain.
/// The renderer must upmix a mono source to [sample, sample] before summation.
/// [sample, 0] is a left-only stereo signal, not an implicit mono format.
pub(super) type Frame = [f32; 2];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Parameters {
    pub threshold: f32,
    pub knee: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
}

impl Default for Parameters {
    fn default() -> Self {
        Self { threshold: -24.0, knee: 30.0, ratio: 12.0, attack: 0.003, release: 0.25 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProcessError {
    SampleRate,
    BlockSize,
    Parameters,
    NonFiniteInput,
}

impl Parameters {
    pub(super) fn validate(self) -> Result<(), ProcessError> {
        for (value, min, max) in [
            (self.threshold, -100.0, 0.0),
            (self.knee, 0.0, 40.0),
            (self.ratio, 1.0, 20.0),
            (self.attack, 0.0, 1.0),
            (self.release, 0.0, 1.0),
        ] {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(ProcessError::Parameters);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct Curve {
    threshold: f64,
    knee: f64,
    inverse_ratio: f64,
    makeup: f64,
}

impl Curve {
    fn new(params: Parameters) -> Self {
        let mut curve = Self {
            threshold: f64::from(params.threshold),
            knee: f64::from(params.knee),
            inverse_ratio: 1.0 / f64::from(params.ratio),
            makeup: 1.0,
        };
        // (1 / full_range_gain)^0.6, evaluated in dB to avoid a reciprocal.
        curve.makeup = db_to_linear(-0.6 * curve.output_db(0.0));
        curve
    }

    fn output_db(self, input_db: f64) -> f64 {
        if self.inverse_ratio == 1.0 {
            return input_db;
        }
        let over = input_db - self.threshold;
        if over <= 0.0 {
            input_db
        } else if self.knee > 0.0 && over < self.knee {
            input_db + (self.inverse_ratio - 1.0) * over * over / (2.0 * self.knee)
        } else {
            self.threshold + self.knee * (1.0 + self.inverse_ratio) * 0.5
                + (over - self.knee) * self.inverse_ratio
        }
    }

    fn attenuation(self, peak: f64) -> f64 {
        if peak < 0.0001 {
            return 1.0;
        }
        let input_db = 20.0 * peak.log10();
        db_to_linear(self.output_db(input_db) - input_db).clamp(0.0, 1.0)
    }
}

fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

// Attack halves a dB-distance over N samples. A reference 20dB transition
// therefore changes gain by 10dB in the specified attack time. Other distances
// follow the same adaptive exponential curve, not a linear slew.
fn envelope_blend(frames: f64) -> f64 {
    if frames == 0.0 { 1.0 } else { -(-std::f64::consts::LN_2 / frames).exp_m1() }
}

fn attack_rate(ratio: f64, blend: f64) -> f64 {
    let log_ratio = ratio.ln();
    if log_ratio == 0.0 {
        blend
    } else {
        // (q-q^(1-blend))/(q-1), written without subtracting close values.
        // Continuous, increasing in q, and bounded by [0,1].
        -(-blend * log_ratio).exp_m1() / -(-log_ratio).exp_m1()
    }
}

fn release_coefficient(frames: f64) -> f64 {
    // With detector=1, gain .1 becomes sqrt(.1) after N samples: a
    // reference -20dB -> -10dB release. Subsample times take one sample.
    (0.1_f64.sqrt() - 0.1) / frames.max(1.0)
}

fn release_rate(ratio: f64, coefficient: f64) -> f64 {
    // R(q)=1+c/q is continuous, decreasing and strictly >1 for every q>0,
    // including when the detector average is still below compressor gain.
    // Preserve the strict lower bound if a tiny c/q rounds away in f64.
    (1.0 + coefficient / ratio).max(f64::from_bits(1.0_f64.to_bits() + 1))
}

/// No shared/global state, locks, device handles or allocations in process().
/// The only allocation is the bounded stereo delay line in new().
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Compressor {
    sample_rate: u32,
    delay: Vec<Frame>,
    cursor: usize,
    delay_whole: usize,
    delay_fraction: f64,
    detector_average: f64,
    compressor_gain: f64,
    reduction_db: f32,
}

impl Compressor {
    pub fn new(sample_rate: u32) -> Result<Self, ProcessError> {
        // Same bounded rate range as the project's existing AudioBuffer.
        if !(3000..=768000).contains(&sample_rate) {
            return Err(ProcessError::SampleRate);
        }
        let delay_frames = f64::from(sample_rate) * LOOK_AHEAD_SECONDS;
        let delay_whole = delay_frames.floor() as usize;
        Ok(Self {
            sample_rate,
            delay: vec![[0.0; 2]; delay_whole + 2],
            cursor: 0,
            delay_whole,
            delay_fraction: delay_frames - delay_whole as f64,
            detector_average: 0.0,
            compressor_gain: 1.0,
            reduction_db: 0.0,
        })
    }

    pub fn reduction_db(&self) -> f32 {
        self.reduction_db
    }

    /// Process one k-rate snapshot and at most one quantum of mixed input.
    /// All fallible checks precede both state changes and output writes.
    pub fn process(
        &mut self,
        input: &[Frame],
        output: &mut [Frame],
        parameters: Parameters,
    ) -> Result<(), ProcessError> {
        if input.is_empty() || input.len() > QUANTUM || input.len() != output.len() {
            return Err(ProcessError::BlockSize);
        }
        parameters.validate()?;
        if input.iter().flatten().any(|sample| !sample.is_finite()) {
            return Err(ProcessError::NonFiniteInput);
        }
        let curve = Curve::new(parameters);
        let attack = envelope_blend(f64::from(parameters.attack) * f64::from(self.sample_rate));
        let release = release_coefficient(f64::from(parameters.release) * f64::from(self.sample_rate));
        let len = self.delay.len();
        let mut metering_gain = 1.0;
        let mut active = false;
        for (frame, out) in input.iter().zip(output.iter_mut()) {
            let peak = f64::from(frame[0].abs().max(frame[1].abs()));
            let attenuation = curve.attenuation(peak);
            let releasing = attenuation > self.compressor_gain;

            // Identity detector curve D(a)=a: continuous, increasing, [0,1].
            self.detector_average += (attenuation - self.detector_average) * attenuation;
            self.detector_average = self.detector_average.clamp(0.0, 1.0);
            // Stop rounding-scale asymptotic drift from alternating attack and
            // release flags forever under a constant input. Real transitions
            // (including a detector still substantially below gain) remain.
            if (attenuation - self.detector_average).abs() <= 8.0 * f64::EPSILON * attenuation {
                self.detector_average = attenuation;
            }
            if releasing {
                let q = self.compressor_gain / self.detector_average;
                self.compressor_gain *= release_rate(q, release);
            } else {
                let q = self.compressor_gain / self.detector_average;
                let rate = attack_rate(q, attack);
                self.compressor_gain += (self.detector_average - self.compressor_gain) * rate;
            }
            self.compressor_gain = self.compressor_gain.clamp(f64::MIN_POSITIVE, 1.0);
            metering_gain = self.compressor_gain * curve.makeup;

            // Write the current input, then read t-6ms. Linear interpolation
            // preserves the fractional delay at rates such as 44100 Hz.
            self.delay[self.cursor] = *frame;
            let recent = (self.cursor + len - self.delay_whole) % len;
            let older = (recent + len - 1) % len;
            let mut delayed_peak = 0.0_f64;
            for (channel, sample) in out.iter_mut().enumerate() {
                let a = f64::from(self.delay[recent][channel]);
                let delayed = if self.delay_fraction == 0.0 {
                    a
                } else {
                    a + (f64::from(self.delay[older][channel]) - a) * self.delay_fraction
                };
                delayed_peak = delayed_peak.max(delayed.abs());
                // Defensive PCM policy: no infinity from a finite f32 input
                // during makeup/attack transients, and no audio soft clipping.
                *sample = (delayed * metering_gain)
                    .clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32;
            }
            active |= peak > 0.0 || delayed_peak > 0.0;
            self.cursor = (self.cursor + 1) % len;
        }
        // Publish the last sample's meter once per block, including makeup
        // as specified by the envelope algorithm. A silent, drained node is 0.
        self.reduction_db = if active { (20.0 * metering_gain.log10()) as f32 } else { 0.0 };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(core: &mut Compressor, input: &[Frame], params: Parameters, chunk: usize) -> Vec<Frame> {
        assert!((1..=QUANTUM).contains(&chunk));
        let mut output = vec![[0.0; 2]; input.len()];
        for (src, dst) in input.chunks(chunk).zip(output.chunks_mut(chunk)) {
            core.process(src, dst, params).unwrap();
        }
        output
    }

    fn hard() -> Parameters {
        Parameters { threshold: -24.0, knee: 0.0, ratio: 12.0, attack: 0.0, release: 0.0 }
    }

    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!((actual - expected).abs() <= tolerance, "actual={actual}, expected={expected}");
    }

    #[test]
    fn unity_ratio_preserves_samples_after_integer_lookahead() {
        let input: Vec<_> = (0..1024).map(|i| [((i * 13 % 31) as f32 - 15.0) / 16.0, 0.0]).collect();
        let mut core = Compressor::new(48000).unwrap();
        let params = Parameters { ratio: 1.0, ..Parameters::default() };
        let output = render(&mut core, &input, params, QUANTUM);
        assert_eq!(core.delay_whole, 288);
        assert_eq!(core.delay_fraction, 0.0);
        assert!(output[..288].iter().all(|frame| *frame == [0.0; 2]));
        assert_eq!(&output[288..], &input[..input.len() - 288]);
        assert_eq!(core.reduction_db(), 0.0);
    }

    #[test]
    fn fractional_lookahead_interpolates_and_preserves_the_tail() {
        let mut input = vec![[0.0; 2]; 600];
        input[0] = [1.0, -0.5];
        let mut core = Compressor::new(44100).unwrap();
        let output = render(&mut core, &input, Parameters { ratio: 1.0, ..hard() }, 7);
        close(f64::from(output[264][0]), 0.4, 1e-6);
        close(f64::from(output[265][0]), 0.6, 1e-6);
        close(f64::from(output[264][1]), -0.2, 1e-6);
        close(f64::from(output[265][1]), -0.3, 1e-6);
        assert!(output[..264].iter().chain(output[266..].iter()).all(|frame| *frame == [0.0; 2]));
        assert_eq!(core.reduction_db(), 0.0);
    }

    #[test]
    fn loud_steady_input_has_the_analytic_hard_knee_gain_and_meter() {
        let mut core = Compressor::new(48000).unwrap();
        let output = render(&mut core, &vec![[1.0, 0.25]; 4096], hard(), QUANTUM);
        // Hard knee: -24+24/12=-22dB; makeup=13.2dB, total=-8.8dB.
        let expected = 10.0_f64.powf(-8.8 / 20.0);
        close(f64::from(output[4095][0]), expected, 1e-6);
        close(f64::from(output[4095][1]), expected * 0.25, 1e-6);
        close(f64::from(core.reduction_db()), -8.8, 1e-5);
        assert!(output[4095][0] < 0.4);
    }

    #[test]
    fn knee_is_continuous_monotone_and_has_the_ratio_slope() {
        let params = Parameters { threshold: -30.0, knee: 20.0, ratio: 4.0, ..hard() };
        let curve = Curve::new(params);
        close(curve.output_db(-40.0), -40.0, 1e-12);
        close(curve.output_db(-30.0), -30.0, 1e-12);
        close(curve.output_db(-10.0), -17.5, 1e-12);
        close(curve.output_db(-6.0) - curve.output_db(-10.0), 1.0, 1e-12);
        for join in [-30.0, -10.0] {
            let a = curve.output_db(join - 1e-6);
            let b = curve.output_db(join + 1e-6);
            assert!(b >= a && b - a < 3e-6);
        }
        let mut previous = curve.output_db(-120.0);
        for i in -1199..=400 {
            let value = f64::from(i) / 10.0;
            let output = curve.output_db(value);
            assert!(output > previous && output <= value + 1e-12);
            previous = output;
        }
    }

    #[test]
    fn attack_and_release_time_parameters_change_the_envelope() {
        let input = vec![[1.0; 2]; 512];
        let mut fast = Compressor::new(8000).unwrap();
        let mut slow = fast.clone();
        render(&mut fast, &input, Parameters { attack: 0.005, ..hard() }, QUANTUM);
        render(&mut slow, &input, Parameters { attack: 0.2, ..hard() }, QUANTUM);
        assert!(fast.compressor_gain < slow.compressor_gain * 0.5);

        let mut fast = Compressor::new(8000).unwrap();
        render(&mut fast, &vec![[1.0; 2]; 4096], hard(), QUANTUM);
        let mut slow = fast.clone();
        let quiet = vec![[0.001; 2]; 400];
        render(&mut fast, &quiet, Parameters { release: 0.005, ..hard() }, QUANTUM);
        render(&mut slow, &quiet, Parameters { release: 0.2, ..hard() }, QUANTUM);
        assert!(fast.compressor_gain > 0.99);
        assert!(slow.compressor_gain < 0.2);
        assert!(fast.compressor_gain > slow.compressor_gain * 4.0);
    }

    #[test]
    fn reference_twenty_db_steps_change_ten_db_in_attack_or_release_time() {
        for rate in [8000, 48000] {
            let duration = 0.01;
            let frames = (f64::from(rate) * duration) as usize;
            let params = Parameters { threshold: -40.0, ratio: 2.0, attack: duration as f32, ..hard() };
            let mut attack = Compressor::new(rate).unwrap();
            // Isolate the envelope time from the separate detector's startup.
            attack.detector_average = 0.1;
            render(&mut attack, &vec![[1.0; 2]; frames], params, QUANTUM);
            close(20.0 * attack.compressor_gain.log10(), -10.0, 1e-5);

            let mut release = Compressor::new(rate).unwrap();
            release.compressor_gain = 0.1;
            release.detector_average = 1.0;
            render(&mut release, &vec![[0.001; 2]; frames], Parameters { release: duration as f32, ..hard() }, QUANTUM);
            close(20.0 * release.compressor_gain.log10(), -10.0, 1e-5);
        }
        let blend = envelope_blend(128.0);
        let mut previous = 0.0;
        for q in [0.001, 0.1, 0.9, 1.0, 1.1, 2.0, 10.0, 1000.0] {
            let rate = attack_rate(q, blend);
            assert!((0.0..=1.0).contains(&rate) && rate >= previous);
            previous = rate;
        }
    }

    #[test]
    fn release_increases_gain_when_detector_average_is_still_below_it() {
        let params = Parameters { threshold: -40.0, ratio: 2.0, attack: 0.003, release: 0.25, ..hard() };
        let mut core = Compressor::new(8000).unwrap();
        core.process(&[[1.0; 2]], &mut [[0.0; 2]], params).unwrap();
        let previous_gain = core.compressor_gain;
        let previous_average = core.detector_average;
        let second = 1.0_f32 / 81.0;
        // Independently: T=-40,R=2 maps amplitude x to attenuation .1/sqrt(x).
        let attenuation = 0.1 / f64::from(second).sqrt();
        let average = previous_average + (attenuation - previous_average) * attenuation;
        assert!(attenuation > previous_gain && average < previous_gain);
        let increment = (0.1_f64.sqrt() - 0.1) * average / (8000.0 * 0.25);
        core.process(&[[second; 2]], &mut [[0.0; 2]], params).unwrap();
        assert!(core.compressor_gain > previous_gain + 0.00008);
        close(core.compressor_gain, previous_gain + increment, 1e-12);
        close(core.detector_average, average, 1e-12);
    }

    #[test]
    fn release_curve_is_continuous_and_strictly_above_one_on_both_sides() {
        let coefficient = release_coefficient(2000.0);
        let mut previous = f64::INFINITY;
        for q in [0.001, 0.1, 0.999999, 1.0, 1.000001, 1.1, 2.0, 1000.0, 1e80] {
            let rate = release_rate(q, coefficient);
            assert!(rate.is_finite() && rate > 1.0 && rate <= previous);
            previous = rate;
        }
        close(release_rate(1.0, coefficient), 1.0 + coefficient, 1e-15);
        assert!((release_rate(1.0 - 1e-6, coefficient) - release_rate(1.0 + 1e-6, coefficient)).abs() < 1e-9);
    }

    #[test]
    fn zero_and_subsample_release_complete_the_reference_step_in_one_sample() {
        for rate in [8000, 48000] {
            for frames in [0.0, 0.5, 1.0] {
                let params = Parameters { release: (frames / f64::from(rate)) as f32, ..hard() };
                let mut core = Compressor::new(rate).unwrap();
                core.compressor_gain = 0.1;
                core.detector_average = 1.0;
                core.process(&[[0.001; 2]], &mut [[0.0; 2]], params).unwrap();
                close(core.compressor_gain, 0.1_f64.sqrt(), 1e-7);
            }
        }
    }

    #[test]
    fn detector_normalization_only_removes_relative_rounding_drift() {
        let attenuation = Curve::new(hard()).attenuation(1.0);
        let mut core = Compressor::new(48000).unwrap();
        core.compressor_gain = attenuation;
        core.detector_average = attenuation * (1.0 - 4.0 * f64::EPSILON);
        core.process(&[[1.0; 2]], &mut [[0.0; 2]], hard()).unwrap();
        assert_eq!(core.detector_average, attenuation);
        assert_eq!(core.compressor_gain, attenuation);
        core.detector_average = attenuation * (1.0 - 1e-8);
        core.process(&[[1.0; 2]], &mut [[0.0; 2]], hard()).unwrap();
        assert!(core.detector_average < attenuation);
    }

    #[test]
    fn k_rate_parameter_changes_keep_delay_and_envelope_state() {
        let mut core = Compressor::new(48000).unwrap();
        let prefix = render(&mut core, &vec![[1.0; 2]; 200], Parameters { ratio: 1.0, ..hard() }, QUANTUM);
        assert!(prefix.iter().all(|frame| *frame == [0.0; 2]));
        let output = render(&mut core, &vec![[1.0; 2]; 512], hard(), QUANTUM);
        assert!(output[..88].iter().all(|frame| *frame == [0.0; 2]));
        assert!(output[88][0] > 0.0 && output[88][0] < 0.4);
        close(f64::from(output[511][0]), 10.0_f64.powf(-8.8 / 20.0), 1e-6);
        let gain = core.compressor_gain;
        render(&mut core, &[[1.0; 2]], Parameters { ratio: 1.0, release: 0.5, ..hard() }, 1);
        assert!(core.compressor_gain > gain && core.compressor_gain < 0.1);
    }

    #[test]
    fn compression_follows_input_summation_and_links_stereo_channels() {
        let a = vec![[0.4, 0.04]; 4096];
        let b = a.clone();
        let mixed: Vec<_> = a.iter().zip(&b).map(|(a, b)| [a[0] + b[0], a[1] + b[1]]).collect();
        let params = Parameters { threshold: -12.0, ratio: 4.0, ..hard() };
        let mut core = Compressor::new(48000).unwrap();
        let mixed_out = render(&mut core, &mixed, params, QUANTUM);
        let mut equivalent = Compressor::new(48000).unwrap();
        assert_eq!(mixed_out, render(&mut equivalent, &vec![[0.8, 0.08]; 4096], params, QUANTUM));
        let mut separate_a = Compressor::new(48000).unwrap();
        let mut separate_b = Compressor::new(48000).unwrap();
        let a_out = render(&mut separate_a, &a, params, QUANTUM);
        let b_out = render(&mut separate_b, &b, params, QUANTUM);
        let last = mixed_out[4095];
        assert!((last[0] - a_out[4095][0] - b_out[4095][0]).abs() > 0.1);
        close(f64::from(last[1] / last[0]), 0.1, 1e-6);
        let expected_db = (-12.0 + (20.0 * 0.8_f64.log10() + 12.0) / 4.0) + 5.4;
        close(f64::from(last[0]), 10.0_f64.powf(expected_db / 20.0), 1e-6);
    }

    #[test]
    fn chunk_partition_does_not_reset_the_detector_or_delay() {
        let input: Vec<_> = (0..4096).map(|i| {
            let x = ((i * 17 % 113) as f32 - 56.0) / 32.0;
            [x, -0.1 * x]
        }).collect();
        let mut a = Compressor::new(44100).unwrap();
        let mut b = a.clone();
        let output = render(&mut a, &input, Parameters::default(), QUANTUM);
        assert_eq!(output, render(&mut b, &input, Parameters::default(), 7));
        assert_eq!(a, b);
    }

    #[test]
    fn silence_and_finite_extremes_keep_output_and_state_finite() {
        for params in [
            Parameters::default(),
            Parameters { threshold: -100.0, knee: 0.0, ratio: 20.0, attack: 1.0, release: 1.0 },
            Parameters { threshold: 0.0, knee: 40.0, ratio: 1.0, attack: 0.0, release: 0.0 },
        ] {
            let mut core = Compressor::new(768000).unwrap();
            assert_eq!(core.delay.len(), 4610);
            let silence = render(&mut core, &vec![[0.0; 2]; 8192], params, QUANTUM);
            assert!(silence.iter().all(|frame| *frame == [0.0; 2]));
            assert_eq!(core.reduction_db(), 0.0);
            let extremes = vec![[f32::MAX, -f32::MAX]; 8192];
            let output = render(&mut core, &extremes, params, QUANTUM);
            assert!(output.iter().flatten().all(|sample| sample.is_finite()));
            assert!(core.detector_average.is_finite() && core.compressor_gain.is_finite());
            assert!(core.reduction_db().is_finite());
            let silence = render(&mut core, &vec![[0.0; 2]; 8192], params, QUANTUM);
            assert!(silence[4610..].iter().all(|frame| *frame == [0.0; 2]));
            assert_eq!(core.reduction_db(), 0.0);
        }
    }

    #[test]
    fn invalid_blocks_are_atomic_for_both_state_and_output() {
        let mut core = Compressor::new(48000).unwrap();
        render(&mut core, &vec![[0.8; 2]; 128], hard(), QUANTUM);
        let before = core.clone();
        for params in [
            Parameters { threshold: f32::NAN, ..hard() },
            Parameters { knee: 40.1, ..hard() },
            Parameters { ratio: 0.0, ..hard() },
            Parameters { attack: -0.1, ..hard() },
            Parameters { release: f32::INFINITY, ..hard() },
        ] {
            let mut out = [[17.0; 2]; 128];
            assert_eq!(core.process(&[[1.0; 2]; 128], &mut out, params), Err(ProcessError::Parameters));
            assert_eq!(out, [[17.0; 2]; 128]);
            assert_eq!(core, before);
        }
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut input = [[1.0; 2]; 128];
            input[127][1] = bad;
            let mut out = [[17.0; 2]; 128];
            assert_eq!(core.process(&input, &mut out, hard()), Err(ProcessError::NonFiniteInput));
            assert_eq!(out, [[17.0; 2]; 128]);
            assert_eq!(core, before);
        }
        let mut out = [[17.0; 2]; 129];
        assert_eq!(core.process(&[[1.0; 2]; 129], &mut out, hard()), Err(ProcessError::BlockSize));
        assert_eq!(out, [[17.0; 2]; 129]);
        assert_eq!(core, before);
        assert_eq!(core.process(&[[1.0; 2]], &mut [], hard()), Err(ProcessError::BlockSize));
        assert_eq!(core.process(&[], &mut [], hard()), Err(ProcessError::BlockSize));
        assert_eq!(core, before);
    }

    #[test]
    fn node_states_are_independent_and_sample_rate_storage_is_bounded() {
        assert_eq!(Compressor::new(2999), Err(ProcessError::SampleRate));
        assert_eq!(Compressor::new(768001), Err(ProcessError::SampleRate));
        let mut a = Compressor::new(48000).unwrap();
        let b = Compressor::new(48000).unwrap();
        let pristine = b.clone();
        render(&mut a, &vec![[1.0; 2]; 1024], hard(), QUANTUM);
        assert_eq!(b, pristine);
        assert_ne!(a.compressor_gain, b.compressor_gain);
        assert_eq!(b.reduction_db(), 0.0);
    }
}
