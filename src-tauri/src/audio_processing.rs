use crate::spatial_audio::SpatialAudioState;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

// Amplifier telemetry (amp_state) smoothed sub crossover cutoffs (f32 bits).
// Statics are only touched from the audio callback (single caller), so no
// extra synchronization is needed beyond the atomic exchange itself.
static AMP_SUB_LOW_SMOOTH: AtomicU32 = AtomicU32::new(20.0f32.to_bits());
static AMP_SUB_HIGH_SMOOTH: AtomicU32 = AtomicU32::new(90.0f32.to_bits());

/// One-pole glide toward `target` (same pattern as EqBank gain smoothing),
/// stored back into `cell`. Keeps filter coefficient changes below the
/// audible-click threshold when the mod changes subsonic/lpf settings.
fn amp_glide(cell: &AtomicU32, target: f32) -> f32 {
    let current = f32::from_bits(cell.load(Ordering::Relaxed));
    let next = current + (target - current) * 0.1;
    cell.store(next.to_bits(), Ordering::Relaxed);
    next
}

// ISO 266:1997 standard center frequencies for audio analysis
// These frequencies correspond to common 1/3-octave band centers used in professional audio EQ
// Band 15 is reserved for master volume control
pub(crate) const EQ_BAND_FREQUENCIES: [f32; 15] = [
    60.0,     // Sub-bass (20-60Hz)
    150.0,    // Bass (60-250Hz)  
    300.0,    // Low mids (250-500Hz)
    600.0,    // Mids (500Hz-1kHz)
    1000.0,   // Upper mids (1-2kHz)
    2400.0,   // Presence (2-4kHz)
    4000.0,   // Brilliance (4-6kHz)
    6000.0,   // Air (6-8kHz)
    8000.0,   // High air (8-10kHz)
    10000.0,  // Ultra high (10-12kHz)
    12000.0,  // Sparkle (12-14kHz)
    14000.0,  // Shine (14-16kHz)
    16000.0,  // Ultra sparkle (16-18kHz)
    18000.0,  // Air band (18-20kHz)
    20000.0,  // Maximum audible frequency
];

#[derive(Clone, Debug)]
pub(crate) struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    pub fn new() -> Self {
        Self { x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }
}

#[derive(Clone)]
pub(crate) struct PeakingFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    current_gain_db: f32, // Tracks last applied gain for dirty-flag state reset
}

impl PeakingFilter {
    pub fn new(freq: f32, gain_db: f32, q: f32, sample_rate: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0_inv = 1.0 / (1.0 + alpha / a);
        Self {
            b0: b0 * a0_inv,
            b1: b1 * a0_inv,
            b2: b2 * a0_inv,
            a1: -2.0 * cos_w0 * a0_inv,
            a2: (1.0 - alpha / a) * a0_inv,
            current_gain_db: gain_db, // Store initial gain for dirty flag comparison
        }
    }

    pub fn update_gain(&mut self, gain_db: f32, q: f32, sample_rate: f32, freq: f32) {
        self.current_gain_db = gain_db; // Keep dirty flag current
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0_inv = 1.0 / (1.0 + alpha / a);
        self.b0 = b0 * a0_inv;
        self.b1 = b1 * a0_inv;
        self.b2 = b2 * a0_inv;
        self.a1 = -2.0 * cos_w0 * a0_inv;
        self.a2 = (1.0 - alpha / a) * a0_inv;
    }

    #[inline(always)]
    pub fn process_sample(&self, state: &mut BiquadState, input: f32) -> f32 {
        let out = self.b0 * input + self.b1 * state.x1 + self.b2 * state.x2
            - self.a1 * state.y1
            - self.a2 * state.y2;
        state.x2 = state.x1;
        state.x1 = input;
        state.y2 = state.y1;
        state.y1 = out;
        out
    }
}

pub(crate) struct EqBank {
    filters: [PeakingFilter; 15],
    left_states: [BiquadState; 15],
    right_states: [BiquadState; 15],
    // One-pole-smoothed band gains (in dB) so the EQ glides smoothly as the
    // camera moves instead of stepping coefficients every audio callback.
    smoothed_gains: [f32; 15],
    // Amplifier bass boost (amp_state telemetry): one peaking filter at the
    // boost frequency, gain and frequency one-pole smoothed so knob/setting
    // changes never click. Gain 0 = pass-through (mod not connected).
    boost_filter: PeakingFilter,
    boost_left_state: BiquadState,
    boost_right_state: BiquadState,
    smoothed_boost_db: f32,
    smoothed_boost_freq: f32,
    boost_last_built_freq: f32,
}

impl EqBank {
    pub fn new(sample_rate: f32) -> Self {
        let q = 1.5; // Tighter bands: reduces inter-band gain compounding
        let filters: [PeakingFilter; 15] =
            std::array::from_fn(|i| PeakingFilter::new(EQ_BAND_FREQUENCIES[i], 0.0, q, sample_rate));
        Self {
            filters,
            left_states: std::array::from_fn(|_| BiquadState::new()),
            right_states: std::array::from_fn(|_| BiquadState::new()),
            smoothed_gains: [0.0; 15],
            boost_filter: PeakingFilter::new(45.0, 0.0, 1.0, sample_rate),
            boost_left_state: BiquadState::new(),
            boost_right_state: BiquadState::new(),
            smoothed_boost_db: 0.0,
            smoothed_boost_freq: 45.0,
            boost_last_built_freq: 45.0,
        }
    }

    /// Called once per audio block with the amplifier's bass boost setting
    /// (amp_state: bass_boost_db at bass_boost_freq_hz, 0 when inactive).
    /// Smooths both gain and frequency; rebuilds the biquad only when the
    /// smoothed values actually moved (same dirty-flag pattern as update_gains).
    pub fn update_amp_boost(&mut self, target_db: f32, target_freq: f32, sample_rate: f32) {
        self.smoothed_boost_db += (target_db - self.smoothed_boost_db) * 0.1;
        self.smoothed_boost_freq += (target_freq - self.smoothed_boost_freq) * 0.1;

        let gain_changed = (self.smoothed_boost_db - self.boost_filter.current_gain_db).abs() > 0.01;
        let freq_changed = (self.smoothed_boost_freq - self.boost_last_built_freq).abs() > 0.5;
        if gain_changed || freq_changed {
            self.boost_filter.update_gain(
                self.smoothed_boost_db,
                1.0,
                sample_rate,
                self.smoothed_boost_freq,
            );
            self.boost_last_built_freq = self.smoothed_boost_freq;
        }
    }

    pub fn update_gains(&mut self, gains: &[f32; 16], sample_rate: f32) {
        let q = 1.5; // Must match EqBank::new
        // One-pole coefficient — a band gain moves ~10% of the way to its target
        // per callback. This keeps per-band coefficient changes below the 0.01 dB
        // threshold on virtually every frame during movement, so filters glide
        // rather than snap, and the state reset below never produces audible steps.
        let smooth = 0.1 * (48000.0 / sample_rate);
        for (i, filter) in self.filters.iter_mut().enumerate() {
            let target = gains[i];
            let current = self.smoothed_gains[i];
            let stepped = current + (target - current) * smooth;
            self.smoothed_gains[i] = stepped;

            // Only rebuild the biquad when the *smoothed* gain actually moved a
            // meaningful amount (avoid needless work). Do NOT reset the IIR state
            // when coefficients change: dropping state to zero mid-signal creates a
            // worse discontinuity (pop) than the tiny coefficient step, and the
            // one-pole glide keeps steps so small that leftover state is fine —
            // a biquad converges on its own over a few samples.
            let changed = (stepped - filter.current_gain_db).abs() > 0.01;
            if changed {
                filter.update_gain(stepped, q, sample_rate, EQ_BAND_FREQUENCIES[i]);
            }
        }
    }

    #[inline]
    pub fn process_sample(&mut self, mut left: f32, mut right: f32) -> (f32, f32) {
        for i in 0..15 {
            left = self.filters[i].process_sample(&mut self.left_states[i], left);
            right = self.filters[i].process_sample(&mut self.right_states[i], right);
        }
        // Amplifier bass boost (amp_state) — part of the amp's input stage,
        // so it applies in both the flat-EQ and the 3D path.
        let (boost_l, boost_r) = self.process_boost(left, right);
        // Post-EQ gain compensation: -3 dB headroom to prevent inter-band compounding
        // from driving the signal hot before it reaches the clipper stage
        const POST_EQ_COMPENSATION: f32 = 0.707_945_8; // 10^(-3/20) = -3 dB
        (boost_l * POST_EQ_COMPENSATION, boost_r * POST_EQ_COMPENSATION)
    }

    /// Amplifier bass boost only (amp_state). Used in the 3D path where the
    /// 15-band EQ is deliberately skipped — the amp's input-stage boost still
    /// applies. Pass-through when the boost gain is ~0 (mod not connected).
    #[inline]
    pub fn process_boost(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.smoothed_boost_db.abs() < 0.01 {
            return (left, right);
        }
        let l = self.boost_filter.process_sample(&mut self.boost_left_state, left);
        let r = self.boost_filter.process_sample(&mut self.boost_right_state, right);
        (l, r)
    }
}

pub(crate) struct DelayLine {
    buffer: Vec<f32>,
    write_pos: usize,
    // Fractional, smoothed delay length in samples. Kept as a float so small
    // geometry changes slew sub-sample and never make a per-block click.
    current_delay: f32,
    target_delay: f32,
}

impl DelayLine {
    pub fn new(max_delay_samples: usize) -> Self {
        Self {
            buffer: vec![0.0f32; max_delay_samples + 1],
            write_pos: 0,
            current_delay: 0.0,
            target_delay: 0.0,
        }
    }

    pub fn set_delay(&mut self, delay_samples: f32) {
        // Treat the requested delay as a target and glide toward it sample-accurately.
        self.target_delay = delay_samples;
    }

    /// Jump straight to a delay with no glide (used for fixed delays at init).
    pub fn set_delay_immediate(&mut self, delay_samples: usize) {
        self.target_delay = delay_samples as f32;
        self.current_delay = delay_samples as f32;
    }

    pub fn process(&mut self, input: f32) -> f32 {
        self.buffer[self.write_pos] = input;

        // Slew the current fractional delay toward the target with a bounded
        // per-sample step, so any delay change (camera movement, ITD handoff as
        // a speaker pans across the head) glides over hundreds of samples
        // instead of jumping (which clicks). 0.05 samples/sample is ~51 samples
        // per 1024-sample block, matching the per-block clamp in update_delays.
        const MAX_STEP: f32 = 0.05;
        let diff = self.target_delay - self.current_delay;
        self.current_delay += diff.clamp(-MAX_STEP, MAX_STEP);

        self.write_pos = (self.write_pos + 1) % self.buffer.len();

        // Read from a fractional position and linearly interpolate between the two
        // nearest samples. This keeps the delay continuous as it glides.
        let read_continuous: f32 =
            (self.write_pos as f32 + self.buffer.len() as f32 - self.current_delay)
            % self.buffer.len() as f32;
        let i0 = read_continuous.floor() as usize;
        let i1 = (i0 + 1) % self.buffer.len();
        let frac = read_continuous - read_continuous.floor();
        self.buffer[i0] * (1.0 - frac) + self.buffer[i1] * frac
    }
}

// ============================================================================
// Biquad filters used for 3D head-related processing:
//  - high shelf for head shadowing (near/far ear brightness)
//  - Butterworth 2nd-order high-pass / low-pass for the sub crossover
// ============================================================================

#[derive(Clone)]
pub(crate) struct BiquadFilter {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadFilter {
    /// RBJ audio-EQ-cookbook high shelf with slope S=1 (max headroom shelf).
    pub fn high_shelf(gain_db: f32, freq: f32, sample_rate: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let sqrt_a = a.sqrt();
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / 2.0 * (2.0f32).sqrt();
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        Self {
            b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha)) / a0,
            b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha)) / a0,
            a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha) / a0,
        }
    }

    /// RBJ audio-EQ-cookbook low shelf with slope S=1 (max headroom shelf).
    pub fn low_shelf(gain_db: f32, freq: f32, sample_rate: f32) -> Self {
        let a = 10.0_f32.powf(gain_db / 40.0);
        let sqrt_a = a.sqrt();
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / 2.0 * (2.0f32).sqrt();
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        Self {
            b0: (a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha)) / a0,
            b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha)) / a0,
            a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha) / a0,
        }
    }

    /// RBJ audio-EQ-cookbook peaking (bell) filter.
    pub fn peaking(gain_db: f32, freq: f32, q: f32, sample_rate: f32) -> Self {        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// 2nd-order all-pass: flat magnitude, phase rotates -180° over the
    /// sweep. Cascaded sections implement the amp's phase shift control.
    pub fn allpass(freq: f32, q: f32, sample_rate: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - alpha) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 + alpha) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 + alpha) / a0,
        }
    }

    /// 2nd-order Butterworth low-pass at fc.
    pub fn low_pass(fc: f32, sample_rate: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / 2.0 * (2.0f32).sqrt();
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w0) / 2.0) / a0,
            b1: ((1.0 - cos_w0)) / a0,
            b2: ((1.0 - cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// 2nd-order Butterworth high-pass at fc.
    pub fn high_pass(fc: f32, sample_rate: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * fc / sample_rate;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / 2.0 * (2.0f32).sqrt();
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_w0) / 2.0) / a0,
            b1: (-(1.0 + cos_w0)) / a0,
            b2: ((1.0 + cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    #[inline(always)]
    pub fn process(&self, state: &mut BiquadState, input: f32) -> f32 {
        let out = self.b0 * input + self.b1 * state.x1 + self.b2 * state.x2
            - self.a1 * state.y1
            - self.a2 * state.y2;
        state.x2 = state.x1;
        state.x1 = input;
        state.y2 = state.y1;
        state.y1 = out;
        out
    }
}

// Bit in `speaker_blocked_flags` for each processing index (FL, FR, RL, RR, SubL, SubR).
// Matches the mod's bit layout: bit0=driver_main(FL), bit1=driver_rear(RL),
// bit3=driver_sub(SubL), bit4=passenger_main(FR), bit5=passenger_rear(RR),
// bit7=passenger_sub(SubR). The 8 UDP speakers collapse into 6 virtual channels.
const BLOCKED_BIT_INDEX: [u32; 6] = [0, 4, 1, 5, 3, 7];

// Echo (feedback delay) applied to speakers occluded by static objects: the
// source is still heard but slightly smeared/roomy, like sound bouncing off a
// wall. Kept subtle so it stays clearly audible.
const ECHO_DELAY_MS: f32 = 180.0;
const ECHO_MAX_DELAY_MS: f32 = 250.0;
const ECHO_FEEDBACK: f32 = 0.35;
const ECHO_MIX: f32 = 0.35;
// Muffle strength for static-object blocking: a light cut, much milder than a
// closed car door, so the speaker stays audible but a bit duller.
const STATIC_MUFFLE: f32 = 0.4;

pub(crate) struct SpatialAudioProcessor {
    delay_lines_left: [DelayLine; 6],
    delay_lines_right: [DelayLine; 6],
    speaker_eq: [EqBank; 6],
    gains: [(f32, f32); 6],
    // Per-sample running copy of `gains` that ramps linearly toward `gains`
    // across each audio block, so gain changes never step at block boundaries
    // (the "zipper"). `gains` stays the block-end target.
    gains_run: [(f32, f32); 6],
    gains_step: [(f32, f32); 6],
    // Same idea for the tweeter overlay.
    tweeter_run: [(f32, f32); 2],
    tweeter_step: [(f32, f32); 2],
    max_delay: usize,
    last_gains: [(f32, f32); 6],
    last_delays: [usize; 6],
    // Head-shadowing shelves: per-speaker near-ear boost / far-ear cut
    // [i] = (left-ear shelf, right-ear shelf) for speaker i
    head_shadow_left: [BiquadFilter; 6],
    head_shadow_right: [BiquadFilter; 6],
    head_shadow_state_l: [BiquadState; 6],
    head_shadow_state_r: [BiquadState; 6],
    // Sub crossover: mains get high-pass, subs get low-pass
    crossover_hp: [BiquadFilter; 4],
    crossover_hp_state_l: [BiquadState; 4],
    crossover_hp_state_r: [BiquadState; 4],
    crossover_lp: [BiquadFilter; 2],
    crossover_lp_state_l: [BiquadState; 2],
    crossover_lp_state_r: [BiquadState; 2],
    crossover_sub_hp: [BiquadFilter; 2],
    crossover_sub_hp_state_l: [BiquadState; 2],
    crossover_sub_hp_state_r: [BiquadState; 2],
    sub_hp_fc: f32,
    sub_lp_fc: f32,
    // Enclosure voicing (amp_state box model): peaking at the box tuning
    // (ported boom; flat for sealed), a low shelf for rolloff character /
    // firing direction / cabin gain, and an output scale for panel flex and
    // air leaks. Targets glide one-pole per block; biquads rebuild only when
    // the smoothed values move.
    box_peak: BiquadFilter,
    box_peak_state_l: BiquadState,
    box_peak_state_r: BiquadState,
    box_peak_built: (f32, f32, f32), // gain_db, freq, q
    box_shelf: BiquadFilter,
    box_shelf_state_l: BiquadState,
    box_shelf_state_r: BiquadState,
    box_shelf_built: (f32, f32), // gain_db, freq
    box_t_peak_db: f32,
    box_t_peak_fc: f32,
    box_t_shelf_db: f32,
    box_t_shelf_fc: f32,
    box_t_scale: f32,
    box_scale: f32,
    // Phase shift control: cascaded all-pass sections on the sub channel.
    box_ap: [BiquadFilter; 4],
    box_ap_state_l: [BiquadState; 4],
    box_ap_state_r: [BiquadState; 4],
    box_ap_sections: usize,
    // Xmax / sub RMS power overdrive: soft-clip drive on the sub channel
    // when demanded power exceeds what the driver can stay linear through.
    box_over_k: f32,
    // Last-applied head-shadow shelf gains for dirty tracking (reset only on change)
    last_hs_gain_l: [f32; 6],
    last_hs_gain_r: [f32; 6],
    // Smoothed per-speaker blocked muffle factor (0 = clear, 1 = fully muffled)
    muffle_current: [f32; 6],
    // Echo (feedback delay) per speaker for static-object occlusion, smoothed.
    echo_lines_left: [DelayLine; 6],
    echo_lines_right: [DelayLine; 6],
    echo_gain: [f32; 6],
    echo_last_l: [f32; 6],
    echo_last_r: [f32; 6],
    // Optional tweeter overlay: high-passed mono bus panned by the tweeter
    // node positions. Only contributes when a tweeter node was found.
    tweeter_gains: [(f32, f32); 2],
    tweeter_hp: BiquadFilter,
    tweeter_lp: BiquadFilter,
    tweeter_hp_state: BiquadState,
    tweeter_lp_state: BiquadState,
    tweeter_hp_fc: f32,
    tweeter_lp_fc: f32,
    // Rattle clip playback state (mono samples at source rate, looped).
    rattle_samples: Vec<f32>,
    rattle_source_rate: f32,
    device_sample_rate: f32,
rattle_pos: f32,
    // Rattle plays as a one-shot and re-triggers (like a bass thump kicking a
    // panel), rather than a hard loop — a short burst looped back-to-back sounds
    // like a broken machine-gun buzz, not the file.
    rattle_playing: bool,
    rattle_attack: f32,
    rattle_retrigger: f32,
    // Rattle shaping: low-pass dulls the clip (panel-like) and removes harsh
    // high-frequency "blown out" character.
    rattle_lp: BiquadFilter,
    rattle_lp_state: BiquadState,
}

impl SpatialAudioProcessor {
    pub fn new(sample_rate: f32, max_delay_ms: f32) -> Self {
        let max_delay_samples = ((max_delay_ms / 1000.0) * sample_rate) as usize;
        let make_filter = |g: f32| BiquadFilter::high_shelf(g, 1800.0, sample_rate);
        let hs_flat = make_filter(0.0);
        let hp_flat = BiquadFilter::high_pass(90.0, sample_rate);
        let lp_flat = BiquadFilter::low_pass(90.0, sample_rate);
        let hp_sub_flat = BiquadFilter::high_pass(20.0, sample_rate);
        Self {
            delay_lines_left: std::array::from_fn(|_| DelayLine::new(max_delay_samples)),
            delay_lines_right: std::array::from_fn(|_| DelayLine::new(max_delay_samples)),
            speaker_eq: std::array::from_fn(|_| EqBank::new(sample_rate)),
            gains: [(0.25, 0.25); 6],
            gains_run: [(0.25, 0.25); 6],
            gains_step: [(0.0, 0.0); 6],
            tweeter_run: [(0.0, 0.0); 2],
            tweeter_step: [(0.0, 0.0); 2],
            max_delay: max_delay_samples,
            last_gains: [(0.0, 0.0); 6],
            last_delays: [0; 6],
            head_shadow_left: std::array::from_fn(|_| hs_flat.clone()),
            head_shadow_right: std::array::from_fn(|_| hs_flat.clone()),
            head_shadow_state_l: std::array::from_fn(|_| BiquadState::new()),
            head_shadow_state_r: std::array::from_fn(|_| BiquadState::new()),
            crossover_hp: std::array::from_fn(|_| hp_flat.clone()),
            crossover_hp_state_l: std::array::from_fn(|_| BiquadState::new()),
            crossover_hp_state_r: std::array::from_fn(|_| BiquadState::new()),
            crossover_lp: std::array::from_fn(|_| lp_flat.clone()),
            crossover_lp_state_l: std::array::from_fn(|_| BiquadState::new()),
            crossover_lp_state_r: std::array::from_fn(|_| BiquadState::new()),
            crossover_sub_hp: std::array::from_fn(|_| hp_sub_flat.clone()),
            crossover_sub_hp_state_l: std::array::from_fn(|_| BiquadState::new()),
            crossover_sub_hp_state_r: std::array::from_fn(|_| BiquadState::new()),
            sub_hp_fc: 20.0,
            sub_lp_fc: 90.0,
            box_peak: BiquadFilter::peaking(0.0, 45.0, 1.0, sample_rate),
            box_peak_state_l: BiquadState::new(),
            box_peak_state_r: BiquadState::new(),
            box_peak_built: (0.0, 45.0, 1.0),
            box_shelf: BiquadFilter::low_shelf(0.0, 80.0, sample_rate),
            box_shelf_state_l: BiquadState::new(),
            box_shelf_state_r: BiquadState::new(),
            box_shelf_built: (0.0, 80.0),
            box_t_peak_db: 0.0,
            box_t_peak_fc: 45.0,
            box_t_shelf_db: 0.0,
            box_t_shelf_fc: 80.0,
            box_t_scale: 1.0,
            box_scale: 1.0,
            box_ap: std::array::from_fn(|_| BiquadFilter::allpass(60.0, 0.7, sample_rate)),
            box_ap_state_l: std::array::from_fn(|_| BiquadState::new()),
            box_ap_state_r: std::array::from_fn(|_| BiquadState::new()),
            box_ap_sections: 0,
            box_over_k: 1.0,
            last_hs_gain_l: [0.0; 6],
            last_hs_gain_r: [0.0; 6],
            muffle_current: [0.0; 6],
            echo_lines_left: std::array::from_fn(|_| {
                let mut dl = DelayLine::new(((ECHO_MAX_DELAY_MS / 1000.0) * sample_rate) as usize + 1);
                dl.set_delay_immediate(((ECHO_DELAY_MS / 1000.0) * sample_rate) as usize);
                dl
            }),
            echo_lines_right: std::array::from_fn(|_| {
                let mut dl = DelayLine::new(((ECHO_MAX_DELAY_MS / 1000.0) * sample_rate) as usize + 1);
                dl.set_delay_immediate(((ECHO_DELAY_MS / 1000.0) * sample_rate) as usize);
                dl
            }),
            echo_gain: [0.0; 6],
            echo_last_l: [0.0; 6],
            echo_last_r: [0.0; 6],
            tweeter_gains: [(0.0f32, 0.0f32); 2],
            tweeter_hp: BiquadFilter::high_pass(2500.0, sample_rate),
            tweeter_lp: BiquadFilter::low_pass(20000.0, sample_rate),
            tweeter_hp_state: BiquadState::new(),
            tweeter_lp_state: BiquadState::new(),
            tweeter_hp_fc: 2500.0,
            tweeter_lp_fc: 20000.0,
            rattle_samples: Vec::new(),
            rattle_source_rate: 0.0,
            device_sample_rate: sample_rate,
            rattle_pos: 0.0,
            rattle_playing: false,
            rattle_attack: 0.0,
            rattle_retrigger: 0.0,
            rattle_lp: BiquadFilter::low_pass(1200.0, sample_rate),
            rattle_lp_state: BiquadState::new(),
        }
    }

    pub fn update_delays(&mut self, distances: &[f32; 6], sample_rate: f32, itd_enabled: bool) {
        let speed_of_sound = 343.0;
        let max_dist = distances.iter().cloned().fold(0.0f32, f32::max);
        let max_delay_samples = ((max_dist / speed_of_sound) * sample_rate) as usize;
        let max_delay_delta = ((0.001 * sample_rate) as i32).max(1);
        // Max interaural time difference ~0.7 ms (sound at 90° off-axis)
        let max_itd_samples = (0.0007 * sample_rate) as usize;

        for (i, &dist) in distances.iter().enumerate() {
            let delay_samples = (dist / speed_of_sound * sample_rate) as usize;
            let target_delay = max_delay_samples.saturating_sub(delay_samples).min(self.max_delay);
            let current_delay = self.last_delays[i];
            let clamped = (target_delay as i32 - current_delay as i32)
                .clamp(-max_delay_delta, max_delay_delta);
            let smoothed = (current_delay as i32 + clamped) as usize;
            self.last_delays[i] = smoothed;

            // ITD: offset the two ears based on the speaker's left/right position.
            // pan derived from smoothed gains: pan>0 → source to the RIGHT → left ear
            // is farther → left ear delayed; pan<0 → source left → right ear delayed.
            let (gl, gr) = self.gains[i];
            let gsum = gl + gr;
            let pan = if gsum > 0.001 { ((gr - gl) / gsum).clamp(-1.0, 1.0) } else { 0.0 };
            let itd = if itd_enabled {
                (pan.abs() * max_itd_samples as f32).min(max_itd_samples as f32)
            } else {
                0.0
            };
            // Split the interaural delay across the two ears with one side carrying
            // the asymmetry. As a speaker pans across the head (pan → 0 → sign flip),
            // the per-ear delays cross through a shared, continuous value instead of
            // jumping the whole ITD from one ear to the other. That handoff is what
            // clicked when looking around; no audible change to the steady-state stereo.
            let delay_center = smoothed as f32 + itd * 0.5;
            // pan>0 (source to the RIGHT): right ear is NEARER → less delay, left ear more.
            // pan<0 (source left): left ear is NEARER → less delay, right ear more.
            let delay_l = if pan > 0.0 { delay_center + itd * 0.5 } else { delay_center - itd * 0.5 };
            let delay_r = if pan < 0.0 { delay_center + itd * 0.5 } else { delay_center - itd * 0.5 };
            // Keep the delay fractional; the DelayLine interpolates and slews it.
            let delay_l = delay_l.min(self.max_delay as f32).max(0.0);
            let delay_r = delay_r.min(self.max_delay as f32).max(0.0);
            self.delay_lines_left[i].set_delay(delay_l);
            self.delay_lines_right[i].set_delay(delay_r);
        }
    }

    /// Call once per audio callback, after `update_gains_and_eq`, with the number
    /// of frames in the block. Sets up per-sample linear ramps so the running
    /// gains reach this block's targets exactly at the end of the block.
    pub fn begin_block(&mut self, frames: usize) {
        let inv = 1.0 / frames.max(1) as f32;
        for i in 0..6 {
            self.gains_step[i] = (
                (self.gains[i].0 - self.gains_run[i].0) * inv,
                (self.gains[i].1 - self.gains_run[i].1) * inv,
            );
        }
        for t in 0..2 {
            self.tweeter_step[t] = (
                (self.tweeter_gains[t].0 - self.tweeter_run[t].0) * inv,
                (self.tweeter_gains[t].1 - self.tweeter_run[t].1) * inv,
            );
        }
    }

    pub fn update_gains_and_eq(
        &mut self,
        audio_state_3d: &SpatialAudioState,
        distances: &[f32; 6],
        base_eq_gains: &[f32; 16],
        sample_rate: f32,
        _muffle_factor: f32,
    ) {
        let spatial_gains = audio_state_3d.calculate_speaker_gains();
        // Allow gains to change quickly enough that distance changes are audible.
        // At 48kHz with ~1024-sample buffers (~46 callbacks/sec), 0.05 delta means
        // full 0→1 transition takes ~0.43s — fast enough to hear movement.
        let max_gain_delta: f32 = 0.05 * (48000.0 / sample_rate);
        const AIR_ABSORPTION_PER_METER: f32 = 0.05;

        // Per-speaker line-of-sight data from the mod (bit per speaker). Falls back
        // to 0 (nothing blocked) when no UDP packet has arrived with the field.
        let (blocked_flags, static_blocked_flags) = audio_state_3d
            .udp_speaker_positions
            .try_lock()
            .map(|g| (g.blocked_flags, g.static_blocked_flags))
            .unwrap_or((0, 0));
        // One-pole coefficient for per-speaker muffle smoothing. The callback rate
        // scales with sample rate (~1024-sample buffers), so scale accordingly.
        let muffle_smooth: f32 = 0.2 * (48000.0 / sample_rate);

        for (i, &(target_l, target_r)) in spatial_gains.iter().enumerate() {
            let clamped_l = (target_l - self.gains[i].0).clamp(-max_gain_delta, max_gain_delta);
            let clamped_r = (target_r - self.gains[i].1).clamp(-max_gain_delta, max_gain_delta);
            self.gains[i].0 += clamped_l;
            self.gains[i].1 += clamped_r;
            self.last_gains[i] = self.gains[i];

            let mut speaker_gains = *base_eq_gains;
            let distance = distances[i];
            if i >= 4 {
                for j in 6..15 {
                    speaker_gains[j] -= 20.0;
                }
            } else if distance > 1.0 {
                let reduction = (distance - 1.0) * AIR_ABSORPTION_PER_METER;
                for j in 8..15 {
                    speaker_gains[j] -= reduction;
                }
            }
            // Rear voicing: rear speakers fire at the seatback, so gently roll off
            // the highs so fronts stay bright and rears sound a bit "behind".
            if audio_state_3d.is_rear_voicing_enabled() && (i == 2 || i == 3) {
                for j in 6..15 {
                    speaker_gains[j] -= 4.0;
                }
            }
            // Muffle high-mids and highs when heard through closed car body panels.
            // Simulates sound travelling through metal/glass rather than open air.
            // Per-speaker: driven by the actual LOS/collision data, so a blocked
            // speaker gets the same "outside the car, doors closed" cut but only
            // that speaker — an open door or clear line of sight keeps it bright.
            // The global door-state muffle still applies as a fallback layer.
            let bit = 1 << BLOCKED_BIT_INDEX[i];
            let target_muffle = if (blocked_flags & bit) != 0 {
                1.0
            } else if (static_blocked_flags & bit) != 0 {
                STATIC_MUFFLE
            } else {
                0.0
            };
            self.muffle_current[i] += (target_muffle - self.muffle_current[i]) * muffle_smooth;
            // Per-speaker muffling only. The old global door-state muffle forced
            // *every* speaker dull whenever all doors were closed (or a door/missing
            // node read as closed), muffling speakers you could actually see. LOS
            // blocked_flags already capture the real per-speaker occlusion, so a
            // visible speaker stays bright regardless of other doors.
            let local_muffle = self.muffle_current[i];
            if local_muffle > 0.001 {
                if i >= 4 {
                    // Subs are crossed over at ~90 Hz, so the high-band cuts below
                    // barely touch them. Cut the low bands instead so blocking a
                    // sub is clearly audible.
                    for j in 0..4 {
                        speaker_gains[j] -= 9.0 * local_muffle;
                    }
                } else {
                    // Bands 5-9 (2.4kHz–10kHz): moderate cut — still present but dulled
                    for j in 3..10 {
                        speaker_gains[j] -= 10.0 * local_muffle;
                    }
                    // Bands 10-14 (12kHz–20kHz): heavy cut — almost no air/sparkle outside
                    for j in 10..15 {
                        speaker_gains[j] -= 20.0 * local_muffle;
                    }
                }
            }

            // Static-object occlusion adds a feedback-delay echo on top of the
            // muffle: still audible but smeared, like sound arriving off a wall.
            let target_echo = if (static_blocked_flags & bit) != 0 { ECHO_MIX } else { 0.0 };
            self.echo_gain[i] += (target_echo - self.echo_gain[i]) * muffle_smooth;

            self.speaker_eq[i].update_gains(&speaker_gains, sample_rate);
        }

        // Optional tweeter overlay: smooth each tweeter's target gain so it
        // fades in/out like the mains (a skunk missing node → target 0).
        let tweeter_targets = audio_state_3d.calculate_tweeter_gains();
        for t in 0..2 {
            let cl = (tweeter_targets[t].0 - self.tweeter_gains[t].0).clamp(-max_gain_delta, max_gain_delta);
            let cr = (tweeter_targets[t].1 - self.tweeter_gains[t].1).clamp(-max_gain_delta, max_gain_delta);
            self.tweeter_gains[t].0 += cl;
            self.tweeter_gains[t].1 += cr;
        }

        // Tweeter band-pass range (min → max Hz). Rebuild the biquads only when
        // the cutoffs actually change; reset the filter states to avoid a click.
        let t_low = audio_state_3d.get_tweeter_low_freq();
        let t_high = audio_state_3d.get_tweeter_high_freq();
        let t_low = t_low.min(t_high - 100.0).max(100.0);
        if (t_low - self.tweeter_hp_fc).abs() > 1.0 {
            self.tweeter_hp = BiquadFilter::high_pass(t_low, sample_rate);
            self.tweeter_hp_state = BiquadState::new();
            self.tweeter_hp_fc = t_low;
        }
        if (t_high - self.tweeter_lp_fc).abs() > 1.0 {
            self.tweeter_lp = BiquadFilter::low_pass(t_high, sample_rate);
            self.tweeter_lp_state = BiquadState::new();
            self.tweeter_lp_fc = t_high;
        }

        // Sub band range: low = sub high-pass, high = sub low-pass + main high-pass
        // crossover. Rebuild the biquads only when a cutoff changes.
        let mut sub_low = audio_state_3d.get_sub_low_freq();
        let mut sub_high = audio_state_3d.get_sub_high_freq();
        // Amplifier telemetry override (tier 1): while the BeamNG mod feeds
        // amp_state, subsonic_hz and lpf_hz drive the sub crossover cutoffs
        // (one-pole glide, rebuilt biquads only on real change). When telemetry
        // stops, the UI-set values are restored.
        if crate::amp_state::is_receiving() {
            let t_low =
                crate::amp_state::get_f64_eff("subsonic_hz", 20.0).clamp(10.0, 60.0) as f32;
            let t_high = crate::amp_state::get_f64_eff("lpf_hz", 90.0).clamp(30.0, 250.0) as f32;
            sub_low = amp_glide(&AMP_SUB_LOW_SMOOTH, t_low);
            sub_high = amp_glide(&AMP_SUB_HIGH_SMOOTH, t_high);
        } else {
            AMP_SUB_LOW_SMOOTH.store(sub_low.to_bits(), Ordering::Relaxed);
            AMP_SUB_HIGH_SMOOTH.store(sub_high.to_bits(), Ordering::Relaxed);
        }
        let sub_low = sub_low.min(sub_high - 5.0).max(10.0);
        if (sub_high - self.sub_lp_fc).abs() > 1.0 {
            let hp = BiquadFilter::high_pass(sub_high, sample_rate);
            let lp = BiquadFilter::low_pass(sub_high, sample_rate);
            for i in 0..4 {
                self.crossover_hp[i] = hp.clone();
                self.crossover_hp_state_l[i] = BiquadState::new();
                self.crossover_hp_state_r[i] = BiquadState::new();
            }
            for s in 0..2 {
                self.crossover_lp[s] = lp.clone();
                self.crossover_lp_state_l[s] = BiquadState::new();
                self.crossover_lp_state_r[s] = BiquadState::new();
            }
            self.sub_lp_fc = sub_high;
        }
        if (sub_low - self.sub_hp_fc).abs() > 1.0 {
            let hp = BiquadFilter::high_pass(sub_low, sample_rate);
            for s in 0..2 {
                self.crossover_sub_hp[s] = hp.clone();
                self.crossover_sub_hp_state_l[s] = BiquadState::new();
                self.crossover_sub_hp_state_r[s] = BiquadState::new();
            }
            self.sub_hp_fc = sub_low;
        }

        // Enclosure voicing (amp_state box model): type, tuning, flex, leaks,
        // polyfill, firing direction, cabin gain, damping factor, break-in.
        self.update_box_voicing(sample_rate);

        // Head shadowing: update per-speaker shelf gains from the smoothed gain pair.
        // Near ear gets a slight high-shelf boost, far ear a cut — emulates the head
        // occluding high frequencies on the far side.  Rebuilds biquads only on change.
        if audio_state_3d.is_head_shadow_enabled() {
            const NEAR_BOOST_DB: f32 = 3.0;
            const FAR_CUT_DB: f32 = 8.0;
            for i in 0..6 {
                let (gl, gr) = self.gains[i];
                let gsum = gl + gr;
                let pan = if gsum > 0.001 { ((gr - gl) / gsum).clamp(-1.0, 1.0) } else { 0.0 };
                let abs = pan.abs();
                let boost = NEAR_BOOST_DB * abs;
                let cut = -FAR_CUT_DB * abs;
                // pan>0 → source right → RIGHT ear near (boost), LEFT ear far (cut)
                let (gain_l, gain_r) = if pan > 0.0 { (cut, boost) } else { (boost, cut) };

                if (gain_l - self.last_hs_gain_l[i]).abs() > 0.05 {
                    self.head_shadow_left[i] = BiquadFilter::high_shelf(gain_l, 1800.0, sample_rate);
                    self.last_hs_gain_l[i] = gain_l;
                }
                if (gain_r - self.last_hs_gain_r[i]).abs() > 0.05 {
                    self.head_shadow_right[i] = BiquadFilter::high_shelf(gain_r, 1800.0, sample_rate);
                    self.last_hs_gain_r[i] = gain_r;
                }
            }
        } else {
            for i in 0..6 {
                if self.last_hs_gain_l[i] != 0.0 {
                    self.head_shadow_left[i] = BiquadFilter::high_shelf(0.0, 1800.0, sample_rate);
                    self.last_hs_gain_l[i] = 0.0;
                }
                if self.last_hs_gain_r[i] != 0.0 {
                    self.head_shadow_right[i] = BiquadFilter::high_shelf(0.0, 1800.0, sample_rate);
                    self.last_hs_gain_r[i] = 0.0;
                }
            }
        }
    }

    /// Enclosure voicing from amp_state (the mod's box + driver jbeam config).
    /// Computes the box response targets (tuning peak, rolloff shelf, output
    /// scale for flex/leaks), glides them one-pole per block and rebuilds the
    /// biquads only when the smoothed values actually move. Flat/unity when
    /// the mod is not connected.
    fn update_box_voicing(&mut self, sample_rate: f32) {
        use crate::amp_state;

        let (peak_db, peak_fc, peak_q, shelf_db, shelf_fc, scale) = if amp_state::is_receiving() || amp_state::overrides_active() {
            let etype = amp_state::get_str_eff("enclosure_type", "ported").to_lowercase();
            let vbox = amp_state::get_f64_eff("enclosure_l", 85.0).max(10.0);
            let fs = amp_state::get_f64_eff("fs_hz", 32.0).clamp(15.0, 60.0);
                let vas = amp_state::get_f64_eff("vas_l", 45.0).max(5.0);
                let qts = amp_state::get_f64_eff("qts", 0.42).clamp(0.2, 0.9);
            let fb = amp_state::get_f64_eff("port_tune_hz", 34.0).clamp(22.0, 50.0);
            let stiff = amp_state::get_f64_eff("suspension_stiffness", 1.0).clamp(0.7, 1.3);
            let breakin = amp_state::get_f64_eff("breakin_pct", 0.0).clamp(0.0, 100.0);
            let damping = amp_state::get_f64_eff("damping_factor", 150.0).clamp(10.0, 1000.0);
            let polyfill = amp_state::get_f64_eff("polyfill_pct", 0.0).clamp(0.0, 100.0);
            let bracing = amp_state::get_f64_eff("bracing_pct", 20.0).clamp(0.0, 100.0);
            let thick = amp_state::get_f64_eff("enclosure_thick_mm", 19.0).clamp(6.0, 40.0);
            let leak = amp_state::get_f64_eff("leak_pct", 0.0).clamp(0.0, 100.0);
            let facing = amp_state::get_str_eff("sub_facing", "back").to_lowercase();
            let cabin = amp_state::get_f64_eff("cabin_gain_db", 6.0).clamp(0.0, 12.0);

            // Break-in relaxes the suspension: effective Fs drifts down over
            // the first playing hours, scaled by the configured stiffness.
            let fs_eff = fs * (1.0 - 0.06 * breakin / 100.0) * stiff;

            let (peak_db, box_fc, shelf_db) =
                if etype.contains("port") || etype.contains("band") {
                    // Ported / bandpass: output boom at the tuning frequency;
                    // polyfill damps the port peak and "acts larger" below tune.
                    let peak = 5.0 - polyfill * 0.02;
                    let shelf = -3.0 + polyfill * 0.03;
                    (peak, fb, shelf)
                } else {
                    // Sealed / free-air / infinite baffle: no tuning peak, the
                    // system resonance rises with box size: Fc = Fs*sqrt(1+Vas/Vb).
                    let fc = (fs_eff * (1.0 + vas / vbox).sqrt()).clamp(25.0, 60.0);
                    let shelf = -5.0 + polyfill * 0.04;
                    (0.0, fc, shelf)
                };
            let _ = fs_eff; // reserved for future excursion modeling

            // Firing direction tilts the low shelf (rear-firing into a hatch
            // booms, forward-firing is tighter).
            let facing_db: f64 = match facing.as_str() {
                "back" => 2.5,
                "forward" => -2.5,
                _ => 0.0,
            };
            // Cabin gain folds into the same low shelf (scaled down to keep
            // the combined shelf sane).
            let shelf_total = shelf_db + facing_db + cabin * 0.6;

            // Panel flex wastes energy: thin, unbraced boxes lose output.
            // Material stiffness scales the flex (steel/alu barely flex, birch
            // ply less than raw MDF). Air leaks bleed pressure: ported boxes
            // suffer most below tune.
            let mat = amp_state::get_str_eff("enclosure_mat", "mdf").to_lowercase();
            let mat_flex = if mat.contains("steel") || mat.contains("alu") {
                0.3
            } else if mat.contains("birch") || mat.contains("ply") {
                0.6
            } else {
                1.0 // mdf and unknown = full flex
            };
            let flex = (1.0 - bracing / 100.0)
                * ((25.0 - thick) / 25.0).clamp(0.0, 1.0)
                * mat_flex;
            let scale = (1.0 - flex * 0.5 - leak / 100.0 * 0.7).clamp(0.3, 1.0);

            (
                peak_db as f32,
                box_fc as f32,
                ((1.1 * (150.0 / damping).sqrt()).clamp(0.6, 1.6) * (qts / 0.42).clamp(0.7, 1.4)) as f32,
                shelf_total as f32,
                box_fc as f32,
                scale as f32,
            )
        } else {
            (0.0, 45.0, 1.0, 0.0, 80.0, 1.0)
        };

        // One-pole glide toward the targets
        self.box_t_peak_db += (peak_db - self.box_t_peak_db) * 0.1;
        self.box_t_peak_fc += (peak_fc - self.box_t_peak_fc) * 0.1;
        self.box_t_shelf_db += (shelf_db - self.box_t_shelf_db) * 0.1;
        self.box_t_shelf_fc += (shelf_fc - self.box_t_shelf_fc) * 0.1;
        self.box_t_scale += (scale - self.box_t_scale) * 0.1;

        let peak_moved = (self.box_t_peak_db - self.box_peak_built.0).abs() > 0.01
            || (self.box_t_peak_fc - self.box_peak_built.1).abs() > 0.5
            || (peak_q - self.box_peak_built.2).abs() > 0.01;
        if peak_moved {
            self.box_peak = BiquadFilter::peaking(
                self.box_t_peak_db,
                self.box_t_peak_fc,
                peak_q,
                sample_rate,
            );
            self.box_peak_built = (self.box_t_peak_db, self.box_t_peak_fc, peak_q);
        }

        let shelf_moved = (self.box_t_shelf_db - self.box_shelf_built.0).abs() > 0.01
            || (self.box_t_shelf_fc - self.box_shelf_built.1).abs() > 0.5;
        if shelf_moved {
            self.box_shelf =
                BiquadFilter::low_shelf(self.box_t_shelf_db, self.box_t_shelf_fc, sample_rate);
            self.box_shelf_built = (self.box_t_shelf_db, self.box_t_shelf_fc);
        }

        self.box_scale = self.box_t_scale;

        // Phase shift control: cascade N all-pass sections on the sub channel
        // (45° per section at 60 Hz). Flat magnitude, shifts sub vs mains.
        let phase = amp_state::get_f64_eff("phase_deg", 0.0).clamp(0.0, 180.0);
        let sections = ((phase / 45.0).round() as usize).min(4);
        if sections != self.box_ap_sections {
            for s in 0..4 {
                self.box_ap[s] = BiquadFilter::allpass(60.0, 0.7, sample_rate);
            }
            self.box_ap_sections = sections;
        }

        // Xmax / sub RMS power overdrive: when demanded power per sub exceeds
        // what the driver can stay linear through (RMS rating), output
        // distorts instead of getting louder; beyond peak rating it gets
        // harsh. Lower Xmax = less clean headroom for the same power.
        let p_per_sub = amp_state::get_last_p_per_sub();
        let sub_rms = amp_state::get_f64_eff("sub_rms_w", 750.0).max(100.0);
        let sub_peak = amp_state::get_f64_eff("sub_peak_w", 1500.0).max(sub_rms);
        let xmax = amp_state::get_f64_eff("xmax_mm", 16.0).clamp(4.0, 40.0);
        let xmax_f = (16.0 / xmax).clamp(0.5, 2.0);
        let over = ((p_per_sub - sub_rms) / sub_rms).clamp(0.0, 1.0);
        let harsh = ((p_per_sub - sub_peak) / sub_peak).clamp(0.0, 1.0) * 0.7;
        let k = (1.0 + 3.0 * over * xmax_f + 2.0 * harsh) as f32;
        self.box_over_k += (k - self.box_over_k) * 0.1;
    }

    pub fn process(
        &mut self,
        left: f32,
        right: f32,
        audio_state_3d: &SpatialAudioState,
    ) -> (f32, f32) {
        let mut out_left  = 0.0f32;
        let mut out_right = 0.0f32;

        let crossover_on = audio_state_3d.is_sub_crossover_enabled();
        let head_shadow_on = audio_state_3d.is_head_shadow_enabled();

// Rattle clip: sample + amount computed once per process() call, then
        // injected into the sub channels so it inherits the subwoofers' position
        // panning and distance attenuation.
        // DISABLED: the sub rattle clip caused audible crackle/popping artifacts,
        // especially during heavy bass, and was removed from the audio path.
        // The clip is still not loaded (see lib.rs), so this stays silent.
        let mut rattle_sample = 0.0f32;
        let mut rattle_amount = 0.0f32;
        if false && !self.rattle_samples.is_empty() {
            rattle_sample = self.read_rattle_sample();
            let sub_blocked = self.muffle_current[4].max(self.muffle_current[5]);
            if sub_blocked > 0.05 {
                let rattle_volume = audio_state_3d.get_rattle_volume();
                let rattle_threshold = audio_state_3d.get_rattle_threshold();
                let sub_level = crate::visualizer::get_visualizer_bands()
                    .get_sub()
                    .clamp(0.0, 1.0);
                // Rattle kicks in once the sub band passes the threshold and
                // ramps up to full by sub = 1.0. 0.5 compensates for injecting
                // into both sub channels.
                let sub_scaled = if sub_level <= rattle_threshold {
                    0.0
                } else {
                    ((sub_level - rattle_threshold) / (1.0 - rattle_threshold)).clamp(0.0, 1.0)
                };
                rattle_amount = sub_blocked * rattle_volume * sub_scaled * 0.5;
            }
        }

        // Advance the per-sample gain ramps (see begin_block).
        for i in 0..6 {
            self.gains_run[i].0 += self.gains_step[i].0;
            self.gains_run[i].1 += self.gains_step[i].1;
        }
        for t in 0..2 {
            self.tweeter_run[t].0 += self.tweeter_step[t].0;
            self.tweeter_run[t].1 += self.tweeter_step[t].1;
        }

        // Amplifier telemetry sub factor (remote knob, thermal, fuse, driver
        // count/sensitivity/area/BL, supply sag, compression). Applied at the
        // FINAL summation — after the headroom normalization below computes
        // its scale from the plain speaker gains — so every level change
        // stays fully audible instead of being normalized away.
        let amp_sub = audio_state_3d.amp_sub_gain_factor();

        for (i, eq) in self.speaker_eq.iter_mut().enumerate() {
            let dl = self.delay_lines_left[i].process(left);
            let dr = self.delay_lines_right[i].process(right);
            let (mut el, mut er) = eq.process_sample(dl, dr);

            // Sub crossover: mains (0..3) high-passed at 90 Hz to stay off the subs,
            // subs (4..5) low-passed at 90 Hz so they carry only the low end.
            if crossover_on {
                if i < 4 {
                    el = self.crossover_hp[i].process(&mut self.crossover_hp_state_l[i], el);
                    er = self.crossover_hp[i].process(&mut self.crossover_hp_state_r[i], er);
                } else {
                    let s = i - 4;
                    el = self.crossover_sub_hp[s].process(&mut self.crossover_sub_hp_state_l[s], el);
                    er = self.crossover_sub_hp[s].process(&mut self.crossover_sub_hp_state_r[s], er);
                    el = self.crossover_lp[s].process(&mut self.crossover_lp_state_l[s], el);
                    er = self.crossover_lp[s].process(&mut self.crossover_lp_state_r[s], er);
                }
            }

            // Enclosure voicing (amp_state box model): tuning peak, rolloff
            // shelf (type / polyfill / facing / cabin gain) and the flex/leak
            // output scale — always applied to the sub channels, the box is
            // physical regardless of the crossover toggle.
            if i >= 4 {
                // Phase shift: cascaded all-pass sections (flat magnitude)
                for a in 0..self.box_ap_sections {
                    el = self.box_ap[a].process(&mut self.box_ap_state_l[a], el);
                    er = self.box_ap[a].process(&mut self.box_ap_state_r[a], er);
                }
                el = self.box_peak.process(&mut self.box_peak_state_l, el);
                er = self.box_peak.process(&mut self.box_peak_state_r, er);
                el = self.box_shelf.process(&mut self.box_shelf_state_l, el);
                er = self.box_shelf.process(&mut self.box_shelf_state_r, er);
                // Xmax / RMS power overdrive: soft-clip the sub when pushed
                // beyond the driver's linear power handling.
                let k = self.box_over_k;
                if k > 1.02 {
                    let drive = k - 1.0;
                    el = el / (1.0 + drive * el * el).sqrt();
                    er = er / (1.0 + drive * er * er).sqrt();
                }
                el *= self.box_scale;
                er *= self.box_scale;
            }

            // Head shadowing: apply the near/far ear shelves BEFORE routing so each
            // ear's contribution is brightened/dulled relative to the source side.
            if head_shadow_on {
                el = self.head_shadow_left[i].process(&mut self.head_shadow_state_l[i], el);
                er = self.head_shadow_right[i].process(&mut self.head_shadow_state_r[i], er);
            }

            // Static-object occlusion: feedback-delay echo. The line always runs
            // (decaying), and the smoothed gain crossfades between dry and the
            // delayed/wet signal so the effect ramps in without clicks.
            let delayed_l =
                self.echo_lines_left[i].process(el + ECHO_FEEDBACK * self.echo_last_l[i]);
            let delayed_r =
                self.echo_lines_right[i].process(er + ECHO_FEEDBACK * self.echo_last_r[i]);
            self.echo_last_l[i] = delayed_l;
            self.echo_last_r[i] = delayed_r;
            if self.echo_gain[i] > 0.001 {
                el = el * (1.0 - self.echo_gain[i]) + delayed_l * self.echo_gain[i];
                er = er * (1.0 - self.echo_gain[i]) + delayed_r * self.echo_gain[i];
            }

            // Route stereo input based on speaker position rather than collapsing to mono.
            // Subwoofers (i >= 4) are non-directional and use mono. All other speakers
            // blend L/R input proportional to their pan position so that left-side speakers
            // draw mostly from the left input — preserving channel separation and preventing
            // left gain from bleeding into the right output through a shared mono mix.
            let mut signal = if i >= 4 {
                // Subwoofers are mono (no spatial L/R)
                (el + er) * 0.5
            } else {
                // Blend L/R input signal proportional to the speaker's pan position.
                // Left-panned speakers draw mostly from left input → preserves channel separation.
                let total = self.gains_run[i].0 + self.gains_run[i].1;
                if total > 0.001 {
                    el * (self.gains_run[i].0 / total) + er * (self.gains_run[i].1 / total)
                } else {
                    (el + er) * 0.5
                }
            };

            // Panel rattle rides the sub channels, so it is positioned at the
            // subwoofers and fades with distance like the rest of the car.
            if i >= 4 && rattle_amount > 0.001 {
                signal += rattle_sample * rattle_amount;
            }

            // Amp telemetry factor multiplies the sub speakers at the very
            // end, outside the headroom normalization path.
            let amp_mul: f32 = if i >= 4 { amp_sub } else { 1.0 };
            out_left  += signal * self.gains_run[i].0 * amp_mul;
            out_right += signal * self.gains_run[i].1 * amp_mul;
        }

        // Tweeter overlay: high-pass the mono input and pan it by the (optional)
        // tweeter node positions, layered on top of the mains. Zero-gain when no
        // tweeter node exists, so this is silent unless the vehicle has them.
        let tw_l = self.tweeter_run[0].0 + self.tweeter_run[1].0;
        let tw_r = self.tweeter_run[0].1 + self.tweeter_run[1].1;
        if tw_l.abs() > 0.0001 || tw_r.abs() > 0.0001 {
            let mut tw_mono =
                self.tweeter_hp.process(&mut self.tweeter_hp_state, (left + right) * 0.5);
            tw_mono = self.tweeter_lp.process(&mut self.tweeter_lp_state, tw_mono);
            out_left += tw_mono * tw_l;
            out_right += tw_mono * tw_r;
        }

        // Dynamic headroom normalization: bound the summed speaker output so
        // close-in 3D audio (high gains) can't overload and clip, while still
        // preserving relative distance differences.  When speakers are far away
        // the total gain is small, so little/no scaling is applied and distant
        // audio stays audible.
        //
        // This replaces the fixed 0.5 divisor, which the 2.0 output gain in
        // lib.rs cancelled out exactly, leaving the raw 6-speaker sum (up to
        // ~6x the input) un-attenuated — the source of the clipping distortion.
        let mut sum_l: f32 = self.gains_run.iter().map(|g| g.0).sum();
        let mut sum_r: f32 = self.gains_run.iter().map(|g| g.1).sum();
        sum_l += self.tweeter_run[0].0 + self.tweeter_run[1].0;
        sum_r += self.tweeter_run[0].1 + self.tweeter_run[1].1;
        let peak_sum = sum_l.max(sum_r);
        let normalization = 1.0 / (1.0 + peak_sum * 0.6);

        
        let out_left  = out_left  * normalization;
        let out_right = out_right * normalization;
        (out_left, out_right)
    }

    /// Next rattle sample, linearly interpolated and resampled from the source
    /// rate to the device rate. Plays the clip as a one-shot; when it ends it
    /// waits a short pseudo-random gap then re-triggers (a panel rattling as
    /// bass kicks it), so you hear the file play rather than a hard loop.
    fn read_rattle_sample(&mut self) -> f32 {
        let n = self.rattle_samples.len();
        if n == 0 {
            return 0.0;
        }

        if !self.rattle_playing {
            if self.rattle_retrigger > 0.0 {
                self.rattle_retrigger -= 1.0;
                return 0.0;
            }
            self.rattle_pos = 0.0;
            self.rattle_playing = true;
            self.rattle_attack = 0.0;
        }

        let idx = self.rattle_pos;
        let i0 = (idx.floor() as usize) % n;
        let i1 = (i0 + 1) % n;
        let frac = idx - idx.floor();
        let mut v = self.rattle_samples[i0] * (1.0 - frac) + self.rattle_samples[i1] * frac;

        self.rattle_pos += self.rattle_source_rate / self.device_sample_rate;
        if self.rattle_pos >= n as f32 {
            // Clip finished; re-trigger after 0.12–0.30s so it never locks
            // into a fixed fast loop. pos.fract() is a cheap pseudo-random seed.
            self.rattle_playing = false;
            self.rattle_retrigger =
                self.device_sample_rate * (0.12 + 0.18 * self.rattle_pos.fract());
        }

        // ~5ms attack ramps the burst in so re-triggering can't click.
        let attack_samples = (0.005 * self.device_sample_rate).max(1.0);
        self.rattle_attack += 1.0 / attack_samples;
        let attack = self.rattle_attack.min(1.0);
        // Release over the last 15% of the clip so the tail can't click off.
        let remain = (n as f32 - self.rattle_pos) / n as f32;
        let release = (remain / 0.15).min(1.0);

        v = v * attack * release;
        v = self.rattle_lp.process(&mut self.rattle_lp_state, v);
        v
    }
}

pub(crate) struct FftBuffer {
    pub data: Vec<f32>,
    pub idx: usize,
}

impl FftBuffer {
    pub fn new(size: usize) -> Self {
        Self { data: vec![0.0f32; size], idx: 0 }
    }
}

pub(crate) struct FftWorkspace {
    pub windowed: Vec<num_complex::Complex<f32>>,
    window_coeffs: Vec<f32>,
}

impl FftWorkspace {
    pub fn new(fft_size: usize) -> Self {
        // Precompute Hann window coefficients to reduce spectral leakage in visualizer
        // Hann window: w(n) = 0.5 * (1 - cos(2πn/(N-1)))
        let window_coeffs: Vec<f32> = (0..fft_size)
            .map(|i| {
                0.5 * (1.0
                    - (2.0 * std::f32::consts::PI * i as f32 / (fft_size - 1) as f32).cos())
            })
            .collect();
        Self {
            windowed: vec![num_complex::Complex::new(0.0, 0.0); fft_size],
            window_coeffs,
        }
    }

    #[inline]
    pub fn apply_window(&mut self, data: &[f32]) {
        for (i, (&s, &window)) in data.iter().zip(self.window_coeffs.iter()).enumerate() {
            self.windowed[i] = num_complex::Complex::new(s * window, 0.0);
        }
    }
}

pub(crate) struct EventThrottle {
    last_emit: std::time::Instant,
    min_interval: std::time::Duration,
}

impl EventThrottle {
    pub fn new(fps: u32) -> Self {
        Self {
            last_emit: std::time::Instant::now(),
            min_interval: std::time::Duration::from_millis(1000 / fps as u64),
        }
    }

    #[inline]
    pub fn should_emit(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now - self.last_emit >= self.min_interval {
            self.last_emit = now;
            true
        } else {
            false
        }
    }
}

#[inline]
/// Soft limiter for the final output stage. Unity below `knee` (fully
/// transparent for normal material), smooth asymptotic compression above it
/// toward `ceiling`. Replaces hard clipping: a loud peak that overshoots is
/// rounded over a few samples instead of chopped into a flat top, so there is
/// no clipping distortion — just a gentle, inaudible roll-off of the very top
/// of transient peaks.
pub(crate) fn soft_limit(sample: f32, knee: f32, ceiling: f32) -> f32 {
    let a = sample.abs();
    if a <= knee {
        return sample;
    }
    let width = (ceiling - knee).max(0.001);
    let x = (a - knee) / width;
    let y = knee + width * (1.0 - (-x).exp());
    if sample < 0.0 {
        -y
    } else {
        y
    }
}

/// Transparent lookahead peak limiter.  Instead of hard-clipping the waveform
/// (which produces harmonic distortion) or ducking with a slow envelope (which
/// "swallows" frequencies), it looks ahead a few milliseconds, detects
/// overshoot peaks before they leave the delay buffer, and applies just enough
/// smooth gain reduction to keep them at/below the ceiling.  Everything below
/// the ceiling passes through at unity gain, so the tone/spectrum is preserved.
pub(crate) struct PeakLimiter {
    ceiling: f32,
    attack_coeff: f32,
    release_coeff: f32,
    gain_l: f32,
    gain_r: f32,
    lookahead: usize,
    // delay lines (FIFO) holding samples waiting to be output
    delay_l: VecDeque<f32>,
    delay_r: VecDeque<f32>,
    // monotonic-decreasing queues of (abs_value, index) for O(1) window max
    window_l: VecDeque<(f32, usize)>,
    window_r: VecDeque<(f32, usize)>,
    // Peak-hold of the detected level (per channel). Holds the window peak so
    // the limiter gain doesn't ripple at the bass frequency on loud material.
    held_peak_l: f32,
    held_peak_r: f32,
    idx: usize,
}

impl PeakLimiter {
    pub fn new(sample_rate: f32, ceiling: f32) -> Self {
        // ~5ms lookahead catches the attack of transient peaks before clipping.
        let lookahead = ((0.005 * sample_rate) as usize).max(8);
        Self {
            ceiling,
            // attack ~2ms, release ~300ms (one-pole coefficients). The long
            // release keeps the gain target (and the peak-hold that drives it)
            // from riding up and down with every bass beat, which is what the
            // "zipper" on loud material was — the gain pumping at the kick rate
            // because 120ms was short enough to decay between beats. 300ms holds
            // through a whole beat so the gain stays put; only genuine level
            // changes move it. When nothing clips, gain stays at 1.0 and the
            // signal passes through unchanged (no tonal change).
            attack_coeff: 1.0 - (-1.0 / (0.002 * sample_rate)).exp(),
            release_coeff: 1.0 - (-1.0 / (0.300 * sample_rate)).exp(),
            gain_l: 1.0,
            gain_r: 1.0,
            lookahead,
            delay_l: VecDeque::with_capacity(lookahead + 1),
            delay_r: VecDeque::with_capacity(lookahead + 1),
            window_l: VecDeque::with_capacity(lookahead + 1),
            window_r: VecDeque::with_capacity(lookahead + 1),
            held_peak_l: 0.0,
            held_peak_r: 0.0,
            idx: 0,
        }
    }

    #[inline]
    fn push_window(window: &mut VecDeque<(f32, usize)>, value: f32, index: usize) {
        // Maintain monotonic-decreasing queue: drop smaller/equal from the back
        while let Some(&(back, _)) = window.back() {
            if back <= value {
                window.pop_back();
            } else {
                break;
            }
        }
        window.push_back((value, index));
    }

    #[inline]
    fn output_delayed(sample: f32, delay: &mut VecDeque<f32>, lookahead: usize, gain: f32) -> f32 {
        delay.push_back(sample);
        if delay.len() > lookahead {
            delay.pop_front().unwrap_or(0.0) * gain
        } else {
            0.0
        }
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let index = self.idx;
        self.idx += 1;

        // Feed current samples into the lookahead window trackers
        Self::push_window(&mut self.window_l, left.abs(), index);
        Self::push_window(&mut self.window_r, right.abs(), index);

        // Drop window entries older than the lookahead span
        let cutoff = index.saturating_sub(self.lookahead + 1);
        while let Some(&(_, i)) = self.window_l.front() {
            if i < cutoff { self.window_l.pop_front(); } else { break; }
        }
        while let Some(&(_, i)) = self.window_r.front() {
            if i < cutoff { self.window_r.pop_front(); } else { break; }
        }

        // Instantaneous peak within the lookahead window.
        let inst_l = self.window_l.front().map(|(v, _)| *v).unwrap_or(0.0);
        let inst_r = self.window_r.front().map(|(v, _)| *v).unwrap_or(0.0);

        // PEAK-HOLD the detected level. A plain 5 ms window peak rises and falls
        // with the bass waveform: at the crests it's big (gain ducks), at the
        // zero-crossings it's small (gain recovers) — so on loud bass the gain
        // modulated at twice the signal frequency, which is the "zipper" sound
        // that got worse with volume. By holding the peak and only decaying it
        // slowly (on the release time constant), the gain target stays put across
        // the whole cycle/beat and the ton is untouched; below the knee nothing
        // engages and gain remains exactly 1.0 (clean pass-through).
        if inst_l > self.held_peak_l {
            self.held_peak_l = inst_l;
        } else {
            self.held_peak_l += (0.0 - self.held_peak_l) * self.release_coeff;
        }
        if inst_r > self.held_peak_r {
            self.held_peak_r = inst_r;
        } else {
            self.held_peak_r += (0.0 - self.held_peak_r) * self.release_coeff;
        }
        // If the release decay carried it below the current window, snap back up
        // so we never gain more than what's actually passing through.
        if self.held_peak_l < inst_l { self.held_peak_l = inst_l; }
        if self.held_peak_r < inst_r { self.held_peak_r = inst_r; }

        // Soft-knee: reduction ramps in over a range just below the ceiling instead
        // of snapping on at the exact threshold. Under the knee nothing is touched
        // (gain stays 1.0) so clean material passes through unchanged.
        let peak_l = self.held_peak_l;
        let peak_r = self.held_peak_r;
        const KNEE_DB: f32 = 1.5; // 1.5 dB-wide softening region (engagement starts high,
                          // so clean material stays at unity longer)
        let knee_low = self.ceiling * db_to_linear(-KNEE_DB);
        fn soft_limit_target(peak: f32, ceiling: f32, knee_low: f32) -> f32 {
            if peak <= knee_low { return 1.0; }
            if peak >= ceiling {
                ceiling / peak
            } else {
                // Blend linearly from 1.0 (at knee_low) to ceiling/peak (at ceiling)
                let t = (peak - knee_low) / (ceiling - knee_low);
                1.0 + (ceiling / peak - 1.0) * t
            }
        }
        let target_l = soft_limit_target(peak_l, self.ceiling, knee_low);
        let target_r = soft_limit_target(peak_r, self.ceiling, knee_low);

        // Smooth gain: fast attack when reducing, slower release when recovering
        let attack = self.attack_coeff;
        let release = self.release_coeff;
        if target_l < self.gain_l {
            self.gain_l += (target_l - self.gain_l) * attack;
        } else {
            self.gain_l += (target_l - self.gain_l) * release;
        }
        if target_r < self.gain_r {
            self.gain_r += (target_r - self.gain_r) * attack;
        } else {
            self.gain_r += (target_r - self.gain_r) * release;
        }

        // Output the delayed sample (from `lookahead` samples ago) with the
        // gain computed from the peaks that are about to reach the output.
        let out_l = Self::output_delayed(left, &mut self.delay_l, self.lookahead, self.gain_l);
        let out_r = Self::output_delayed(right, &mut self.delay_r, self.lookahead, self.gain_r);
        (out_l, out_r)
    }
}

#[inline]
pub(crate) fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub(crate) fn calculate_magnitude_band(
    fft_output: &[num_complex::Complex<f32>],
    sample_rate: f32,
    fft_size: usize,
    low_freq: f32,
    high_freq: f32,
) -> f32 {
    let bin_size = sample_rate / fft_size as f32;
    let start_bin = (low_freq / bin_size) as usize;
    let end_bin = ((high_freq / bin_size) as usize).min(fft_size / 2);
    if start_bin >= end_bin {
        return 0.0;
    }
    let sum: f32 = fft_output[start_bin..end_bin].iter().map(|c| c.norm()).sum();
    sum / (end_bin - start_bin) as f32
}