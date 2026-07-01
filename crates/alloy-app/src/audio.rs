use alloy_core::Track;
use anyhow::Context;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::fs::File;
use std::io::BufReader;
use std::time::{Duration, Instant};

const PROCESSING_BLOCK_SAMPLES: usize = 1024;

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
        })
    }

    pub fn play(
        &mut self,
        track: Track,
        processor_factories: Vec<Box<dyn SampleBlockProcessorFactory>>,
    ) -> anyhow::Result<()> {
        self.play_from(track, Duration::ZERO, processor_factories)
    }

    pub fn play_from(
        &mut self,
        track: Track,
        offset: Duration,
        processor_factories: Vec<Box<dyn SampleBlockProcessorFactory>>,
    ) -> anyhow::Result<()> {
        let file = File::open(&track.path)
            .with_context(|| format!("failed to open {}", track.path.display()))?;
        let decoder = Decoder::new(BufReader::new(file))
            .with_context(|| format!("failed to decode {}", track.path.display()))?;
        let duration = decoder.total_duration().or(track.duration);
        let source = decoder.convert_samples::<f32>();
        let channels = source.channels();
        let sample_rate = source.sample_rate();
        let source = source.skip_duration(offset);
        let sink = Sink::try_new(&self.handle).context("failed to create audio sink")?;
        let processors = processor_factories
            .into_iter()
            .map(|factory| factory.create(channels, sample_rate))
            .collect::<anyhow::Result<Vec<_>>>()
            .context("failed to create audio processors")?;
        let source = ProcessingSource::new(source, channels, sample_rate, processors);

        sink.set_volume(self.volume);
        sink.append(source);

        if let Some(old_sink) = self.sink.take() {
            old_sink.stop();
        }

        self.sink = Some(sink);
        self.state = PlaybackState::Playing;
        self.current_track = Some(track);
        self.started_at = Some(Instant::now());
        self.base_position = offset;
        self.duration = duration;
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state != PlaybackState::Playing {
            return;
        }
        self.base_position = self.position();
        self.started_at = None;
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.state = PlaybackState::Paused;
    }

    pub fn resume(&mut self) {
        if self.state != PlaybackState::Paused {
            return;
        }
        self.started_at = Some(Instant::now());
        if let Some(sink) = &self.sink {
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
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.5);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
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
        self.current_track.take()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_db_is_unity_gain() {
        assert!((db_to_amp(0.0) - 1.0).abs() < f32::EPSILON);
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
