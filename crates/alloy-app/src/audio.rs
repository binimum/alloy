use alloy_core::Track;
use anyhow::Context;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::time::{Duration, Instant};

const PROCESSING_BLOCK_SAMPLES: usize = 1024;
const PLAY_FADE_DURATION: Duration = Duration::from_millis(180);
const PAUSE_FADE_DURATION: Duration = Duration::from_millis(160);
const SEEK_FADE_DURATION: Duration = Duration::from_millis(70);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub track: Option<Track>,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub volume: f32,
}

pub struct AudioEngine {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    state: PlaybackState,
    current_track: Option<Track>,
    started_at: Option<Instant>,
    base_position: Duration,
    duration: Option<Duration>,
    volume: f32,
    bit_perfect: bool,
    fade: Option<VolumeFade>,
}

impl AudioEngine {
    pub fn new(volume: f32) -> anyhow::Result<Self> {
        let (_stream, handle) =
            OutputStream::try_default().context("failed to open the default audio output")?;
        Ok(Self {
            _stream,
            handle,
            sink: None,
            state: PlaybackState::Stopped,
            current_track: None,
            started_at: None,
            base_position: Duration::ZERO,
            duration: None,
            volume,
            bit_perfect: false,
            fade: None,
        })
    }

    pub fn play(
        &mut self,
        track: Track,
        processor_factories: Vec<Box<dyn SampleBlockProcessorFactory>>,
        bit_perfect: bool,
    ) -> anyhow::Result<()> {
        self.play_from_with_fade(
            track,
            Duration::ZERO,
            processor_factories,
            bit_perfect,
            PLAY_FADE_DURATION,
        )
    }

    pub fn play_from(
        &mut self,
        track: Track,
        offset: Duration,
        processor_factories: Vec<Box<dyn SampleBlockProcessorFactory>>,
        bit_perfect: bool,
    ) -> anyhow::Result<()> {
        self.play_from_with_fade(
            track,
            offset,
            processor_factories,
            bit_perfect,
            SEEK_FADE_DURATION,
        )
    }

    fn play_from_with_fade(
        &mut self,
        track: Track,
        offset: Duration,
        processor_factories: Vec<Box<dyn SampleBlockProcessorFactory>>,
        bit_perfect: bool,
        fade_duration: Duration,
    ) -> anyhow::Result<()> {
        let file = File::open(&track.path)
            .with_context(|| format!("failed to open {}", track.path.display()))?;
        let decoder = Decoder::new(BufReader::new(file))
            .with_context(|| format!("failed to decode {}", track.path.display()))?;
        let duration = decoder.total_duration().or(track.duration);
        let sink = Sink::try_new(&self.handle).context("failed to create audio sink")?;
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let source = decoder.skip_duration(offset);
        let target_volume = if bit_perfect { 1.0 } else { self.volume };
        let fade_duration = if bit_perfect {
            Duration::ZERO
        } else {
            fade_duration
        };

        if bit_perfect {
            sink.set_volume(1.0);
            sink.append(source);
        } else {
            let source = source.convert_samples::<f32>();
            let processors = processor_factories
                .into_iter()
                .map(|factory| factory.create(channels, sample_rate))
                .collect::<anyhow::Result<Vec<_>>>()
                .context("failed to create audio processors")?;
            let source = ProcessingSource::new(source, channels, sample_rate, processors);
            sink.set_volume(if fade_duration.is_zero() {
                target_volume
            } else {
                0.0
            });
            sink.append(source);
        }

        if let Some(old_sink) = self.sink.take() {
            old_sink.stop();
        }

        self.sink = Some(sink);
        self.state = PlaybackState::Playing;
        self.current_track = Some(track);
        self.started_at = Some(Instant::now());
        self.base_position = offset;
        self.duration = duration;
        self.bit_perfect = bit_perfect;
        self.fade = if fade_duration.is_zero() {
            None
        } else {
            Some(VolumeFade::new(
                0.0,
                target_volume,
                fade_duration,
                FadeAction::None,
            ))
        };
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state != PlaybackState::Playing {
            return;
        }
        self.base_position = self.position();
        self.started_at = None;
        if let Some(sink) = &self.sink {
            if self.bit_perfect {
                sink.pause();
            } else {
                let current_volume = self.current_sink_volume();
                self.fade = Some(VolumeFade::new(
                    current_volume,
                    0.0,
                    PAUSE_FADE_DURATION,
                    FadeAction::Pause,
                ));
            }
        }
        self.state = PlaybackState::Paused;
    }

    pub fn resume(&mut self) {
        if self.state != PlaybackState::Paused {
            return;
        }
        self.started_at = Some(Instant::now());
        if let Some(sink) = &self.sink {
            if !self.bit_perfect {
                sink.set_volume(0.0);
                self.fade = Some(VolumeFade::new(
                    0.0,
                    self.volume,
                    PLAY_FADE_DURATION,
                    FadeAction::None,
                ));
            }
            sink.play();
        }
        self.state = PlaybackState::Playing;
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.state = PlaybackState::Stopped;
        self.current_track = None;
        self.started_at = None;
        self.base_position = Duration::ZERO;
        self.duration = None;
        self.bit_perfect = false;
        self.fade = None;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.5);
        if let Some(sink) = &self.sink
            && !self.bit_perfect
        {
            if let Some(fade) = &mut self.fade {
                if fade.action == FadeAction::None {
                    fade.target = self.volume;
                }
            } else {
                sink.set_volume(self.volume);
            }
        }
    }

    pub fn tick(&mut self) {
        let Some(fade) = self.fade else {
            return;
        };

        let volume = fade.volume_at(Instant::now());
        if let Some(sink) = &self.sink {
            sink.set_volume(volume);
        }

        if fade.finished(Instant::now()) {
            if let Some(sink) = &self.sink {
                sink.set_volume(fade.target);
                if fade.action == FadeAction::Pause {
                    sink.pause();
                }
            }
            self.fade = None;
        }
    }

    #[must_use]
    pub fn is_fading(&self) -> bool {
        self.fade.is_some()
    }

    #[must_use]
    pub fn position(&self) -> Duration {
        if self.state == PlaybackState::Playing {
            self.started_at.map_or(self.base_position, |started_at| {
                self.base_position + started_at.elapsed()
            })
        } else {
            self.base_position
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> PlaybackSnapshot {
        PlaybackSnapshot {
            state: self.state,
            track: self.current_track.clone(),
            position: self.position(),
            duration: self.duration,
            volume: self.volume,
        }
    }

    #[must_use]
    pub fn current_track(&self) -> Option<&Track> {
        self.current_track.as_ref()
    }

    #[must_use]
    pub fn state(&self) -> PlaybackState {
        self.state
    }

    pub fn take_finished_track(&mut self) -> Option<Track> {
        if self.state != PlaybackState::Playing {
            return None;
        }

        let sink_finished = self.sink.as_ref().is_some_and(Sink::empty);
        if !sink_finished {
            return None;
        }

        self.state = PlaybackState::Stopped;
        self.started_at = None;
        self.base_position = Duration::ZERO;
        self.duration = None;
        self.sink = None;
        self.fade = None;
        self.current_track.take()
    }

    fn current_sink_volume(&self) -> f32 {
        self.fade
            .map_or(if self.bit_perfect { 1.0 } else { self.volume }, |fade| {
                fade.volume_at(Instant::now())
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FadeAction {
    None,
    Pause,
}

#[derive(Debug, Clone, Copy)]
struct VolumeFade {
    start: f32,
    target: f32,
    started_at: Instant,
    duration: Duration,
    action: FadeAction,
}

impl VolumeFade {
    fn new(start: f32, target: f32, duration: Duration, action: FadeAction) -> Self {
        Self {
            start,
            target,
            started_at: Instant::now(),
            duration,
            action,
        }
    }

    fn volume_at(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return self.target;
        }

        let progress = (now - self.started_at).as_secs_f32() / self.duration.as_secs_f32();
        let eased = ease_out_cubic(progress.clamp(0.0, 1.0));
        self.start + (self.target - self.start) * eased
    }

    fn finished(&self, now: Instant) -> bool {
        now.duration_since(self.started_at) >= self.duration
    }
}

#[derive(Debug, Clone)]
pub struct EqProfile {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands: Vec<EqBand>,
}

#[derive(Debug, Clone)]
pub struct EqBand {
    pub frequency_hz: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Debug, Clone)]
pub struct ReplayGainProfile {
    pub enabled: bool,
    pub gain_db: f32,
    pub peak: Option<f32>,
    pub prevent_clipping: bool,
}

impl ReplayGainProfile {
    #[must_use]
    pub fn effective_amplification(&self) -> f32 {
        if !self.enabled {
            return 1.0;
        }

        let mut amplification = db_to_amp(self.gain_db);
        if self.prevent_clipping
            && let Some(peak) = self.peak
            && peak > 0.0
            && peak * amplification > 1.0
        {
            amplification = 1.0 / peak;
        }
        amplification
    }
}

pub trait SampleBlockProcessor: Send {
    fn process(&mut self, samples: &mut [f32], channels: u16, sample_rate: u32);
}

pub trait SampleBlockProcessorFactory: Send {
    fn create(
        self: Box<Self>,
        channels: u16,
        sample_rate: u32,
    ) -> anyhow::Result<Box<dyn SampleBlockProcessor>>;
}

pub struct EqProcessorFactory {
    profile: EqProfile,
}

impl EqProcessorFactory {
    #[must_use]
    pub fn new(profile: EqProfile) -> Self {
        Self { profile }
    }
}

impl SampleBlockProcessorFactory for EqProcessorFactory {
    fn create(
        self: Box<Self>,
        channels: u16,
        sample_rate: u32,
    ) -> anyhow::Result<Box<dyn SampleBlockProcessor>> {
        Ok(Box::new(EqualizerProcessor::new(
            channels,
            sample_rate,
            &self.profile,
        )))
    }
}

pub struct ReplayGainProcessorFactory {
    profile: ReplayGainProfile,
}

impl ReplayGainProcessorFactory {
    #[must_use]
    pub fn new(profile: ReplayGainProfile) -> Self {
        Self { profile }
    }
}

impl SampleBlockProcessorFactory for ReplayGainProcessorFactory {
    fn create(
        self: Box<Self>,
        _channels: u16,
        _sample_rate: u32,
    ) -> anyhow::Result<Box<dyn SampleBlockProcessor>> {
        Ok(Box::new(ReplayGainProcessor {
            amplification: self.profile.effective_amplification(),
            prevent_clipping: self.profile.prevent_clipping,
        }))
    }
}

struct ReplayGainProcessor {
    amplification: f32,
    prevent_clipping: bool,
}

impl SampleBlockProcessor for ReplayGainProcessor {
    fn process(&mut self, samples: &mut [f32], _channels: u16, _sample_rate: u32) {
        if (self.amplification - 1.0).abs() < f32::EPSILON {
            return;
        }

        for sample in samples {
            let processed = *sample * self.amplification;
            *sample = if self.prevent_clipping {
                processed.clamp(-1.0, 1.0)
            } else {
                processed
            };
        }
    }
}

struct ProcessingSource<S> {
    input: S,
    processors: Vec<Box<dyn SampleBlockProcessor>>,
    pending: Vec<f32>,
    pending_index: usize,
    channels: u16,
    sample_rate: u32,
}

impl<S> ProcessingSource<S>
where
    S: Source<Item = f32>,
{
    fn new(
        input: S,
        channels: u16,
        sample_rate: u32,
        processors: Vec<Box<dyn SampleBlockProcessor>>,
    ) -> Self {
        Self {
            input,
            processors,
            pending: Vec::with_capacity(PROCESSING_BLOCK_SAMPLES),
            pending_index: 0,
            channels,
            sample_rate,
        }
    }

    fn refill(&mut self) -> bool {
        self.pending.clear();
        self.pending_index = 0;

        for _ in 0..PROCESSING_BLOCK_SAMPLES {
            let Some(sample) = self.input.next() else {
                break;
            };
            self.pending.push(sample);
        }

        if self.pending.is_empty() {
            return false;
        }

        for processor in &mut self.processors {
            processor.process(&mut self.pending, self.channels, self.sample_rate);
        }

        true
    }
}

impl<S> Iterator for ProcessingSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pending_index >= self.pending.len() && !self.refill() {
            return None;
        }

        let sample = self.pending[self.pending_index];
        self.pending_index += 1;
        Some(sample)
    }
}

impl<S> Source for ProcessingSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.input.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.input.total_duration()
    }
}

struct EqualizerProcessor {
    filters: Vec<Vec<Biquad>>,
    next_channel: usize,
    preamp: f32,
}

impl EqualizerProcessor {
    fn new(channels: u16, sample_rate: u32, profile: &EqProfile) -> Self {
        let channel_count = usize::from(channels.max(1));
        let filters = (0..channel_count)
            .map(|_| {
                profile
                    .bands
                    .iter()
                    .filter(|band| band.gain_db.abs() > 0.05)
                    .filter_map(|band| Biquad::peaking(sample_rate as f32, band))
                    .collect()
            })
            .collect();
        Self {
            filters,
            next_channel: 0,
            preamp: db_to_amp(profile.preamp_db),
        }
    }
}

impl SampleBlockProcessor for EqualizerProcessor {
    fn process(&mut self, samples: &mut [f32], _channels: u16, _sample_rate: u32) {
        for sample in samples {
            let channel = self.next_channel;
            self.next_channel = (self.next_channel + 1) % self.filters.len().max(1);

            let mut processed = *sample * self.preamp;
            if let Some(filters) = self.filters.get_mut(channel) {
                for filter in filters {
                    processed = filter.process(processed);
                }
            }
            *sample = processed.clamp(-1.0, 1.0);
        }
    }
}

#[derive(Debug, Clone)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn peaking(sample_rate: f32, band: &EqBand) -> Option<Self> {
        if sample_rate <= 0.0 || band.frequency_hz <= 0.0 || band.frequency_hz >= sample_rate / 2.0
        {
            return None;
        }

        let q = band.q.max(0.1);
        let a = 10.0_f32.powf(band.gain_db / 40.0);
        let omega = 2.0 * std::f32::consts::PI * band.frequency_hz / sample_rate;
        let sin = omega.sin();
        let cos = omega.cos();
        let alpha = sin / (2.0 * q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos;
        let a2 = 1.0 - alpha / a;

        Some(Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        })
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        output
    }
}

fn db_to_amp(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_db_is_unity_gain() {
        assert!((db_to_amp(0.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn replay_gain_prevents_clipping_when_peak_would_overflow() {
        let profile = ReplayGainProfile {
            enabled: true,
            gain_db: 6.0,
            peak: Some(0.8),
            prevent_clipping: true,
        };

        assert!((profile.effective_amplification() - 1.25).abs() < 0.001);
    }

    #[test]
    fn biquad_produces_finite_samples() {
        let band = EqBand {
            frequency_hz: 1_000.0,
            gain_db: 4.0,
            q: 1.0,
        };
        let mut biquad = Biquad::peaking(44_100.0, &band).expect("valid band");

        for _ in 0..256 {
            let sample = biquad.process(0.25);
            assert!(sample.is_finite());
        }
    }
}
