//! Playing the console's sound: the session's thread hands PCM to a queue,
//! and one thread of our own owns the output device and drains it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, ErrorKind, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::Error;

/// The frames one request to the device holds: ten milliseconds at 48 kHz.
const FRAMES: u32 = 480;
/// How long the playing thread waits for sound before it checks whether to stop.
const WAIT: Duration = Duration::from_millis(20);
/// The most samples queued before the oldest make way, in each of the two queues: half a
/// second of stereo at 48 kHz.
const QUEUED: usize = 48_000;

/// The thread that plays the sound. Dropping it stops the device.
#[derive(Debug)]
pub(crate) struct Output {
    queue: Arc<Queue>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Output {
    pub(crate) fn open(faults: Arc<Mutex<Vec<String>>>) -> Self {
        let queue = Arc::new(Queue {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        });
        let stopped = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (queue, stopped) = (Arc::clone(&queue), Arc::clone(&stopped));
            move || play(&queue, &stopped, &faults)
        });
        Self {
            queue,
            stopped,
            thread: Some(thread),
        }
    }

    pub(crate) fn queue(&self) -> Arc<Queue> {
        Arc::clone(&self.queue)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.queue.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// The channels and rate of the sound, once the console said.
    shape: Option<(u32, u32)>,
    samples: VecDeque<i16>,
}

/// What the playing thread takes at once: the shape, when the console just gave it, and the samples.
type Batch = (Option<(u32, u32)>, Vec<i16>);

/// What the session's thread puts the sound into.
#[derive(Debug)]
pub(crate) struct Queue {
    state: Mutex<State>,
    ready: Condvar,
}

impl Queue {
    /// The shape of the sound that follows.
    pub(crate) fn set_shape(&self, channels: u32, rate: u32) {
        if let Ok(mut state) = self.state.lock() {
            state.shape = Some((channels, rate));
        }
    }

    /// Queue interleaved 16-bit samples; they play in the order they came.
    pub(crate) fn push(&self, samples: &[i16]) {
        if let Ok(mut state) = self.state.lock() {
            state.samples.extend(samples);
            let over = state.samples.len().saturating_sub(QUEUED);
            state.samples.drain(..over);
        }
        self.ready.notify_one();
    }

    /// The shape and everything queued, waiting a little when nothing is;
    /// `None` once stopped.
    fn next(&self, stopped: &AtomicBool) -> Option<Batch> {
        let mut state = self.state.lock().ok()?;
        while state.samples.is_empty() && !stopped.load(Ordering::Relaxed) {
            state = self.ready.wait_timeout(state, WAIT).ok()?.0;
        }
        if stopped.load(Ordering::Relaxed) {
            return None;
        }
        let shape = state.shape;
        let batch = state.samples.drain(..).collect();
        Some((shape, batch))
    }

    fn wake(&self) {
        self.ready.notify_one();
    }
}

/// Make the device once the sound has a shape, and keep it fed until stopped.
fn play(queue: &Queue, stopped: &AtomicBool, faults: &Arc<Mutex<Vec<String>>>) {
    let mut speaker: Option<(Speaker, (u32, u32))> = None;
    let mut refused: Option<(u32, u32)> = None;
    while let Some((shape, batch)) = queue.next(stopped) {
        let outdated = speaker
            .as_ref()
            .is_none_or(|(_, have)| Some(*have) != shape);
        if let Some(shape) = shape.filter(|shape| outdated && refused != Some(*shape)) {
            match Speaker::open(shape.0, shape.1, faults) {
                Ok(opened) => speaker = Some((opened, shape)),
                Err(error) => {
                    record(faults, &error);
                    refused = Some(shape);
                }
            }
        }
        if let Some((speaker, _)) = speaker.as_mut() {
            speaker.push(&batch);
        }
    }
}

/// One output stream, made, fed and dropped on the thread that made it.
struct Speaker {
    _stream: Stream,
    samples: Arc<Mutex<VecDeque<i16>>>,
}

impl Speaker {
    /// Play interleaved 16-bit samples of `channels` channels at `rate`
    /// frames a second.
    fn open(channels: u32, rate: u32, faults: &Arc<Mutex<Vec<String>>>) -> Result<Self, Error> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| failed("there is no output device"))?;
        let channels = usize::try_from(channels)
            .ok()
            .filter(|channels| *channels > 0)
            .ok_or_else(|| failed("the console sends no channels"))?;
        // We play the console's samples as they come, so the device has to take its exact
        // shape in a format we can write.
        let range = device
            .supported_output_configs()
            .map_err(failed)?
            .find(|range| {
                usize::from(range.channels()) == channels
                    && range.min_sample_rate() <= rate
                    && rate <= range.max_sample_rate()
                    && matches!(range.sample_format(), SampleFormat::I16 | SampleFormat::F32)
            })
            .ok_or_else(|| {
                failed(format!(
                    "the output device cannot play {channels} channels at {rate} Hz"
                ))
            })?;
        let (channels, format) = (range.channels(), range.sample_format());
        let samples = Arc::new(Mutex::new(VecDeque::new()));
        let mut config = StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: BufferSize::Fixed(FRAMES),
        };
        let stream = build(&device, &config, format, &samples, faults)
            .or_else(|source| {
                if source.kind() != ErrorKind::UnsupportedConfig {
                    return Err(source);
                }
                config.buffer_size = BufferSize::Default;
                build(&device, &config, format, &samples, faults)
            })
            .map_err(failed)?;
        stream.play().map_err(failed)?;
        Ok(Self {
            _stream: stream,
            samples,
        })
    }

    fn push(&self, samples: &[i16]) {
        if let Ok(mut queue) = self.samples.lock() {
            queue.extend(samples);
            let over = queue.len().saturating_sub(QUEUED);
            queue.drain(..over);
        }
    }
}

fn build(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    samples: &Arc<Mutex<VecDeque<i16>>>,
    faults: &Arc<Mutex<Vec<String>>>,
) -> Result<Stream, cpal::Error> {
    let queued = Arc::clone(samples);
    let faults = Arc::clone(faults);
    let errors = move |error: cpal::Error| {
        if let Ok(mut faults) = faults.lock() {
            faults.push(error.to_string());
        }
    };
    match format {
        SampleFormat::I16 => device.build_output_stream(
            *config,
            move |data: &mut [i16], _| fill(data, &queued),
            errors,
            None,
        ),
        SampleFormat::F32 => device.build_output_stream(
            *config,
            move |data: &mut [f32], _| fill(data, &queued),
            errors,
            None,
        ),
        _ => Err(cpal::Error::new(ErrorKind::UnsupportedConfig)),
    }
}

/// Fill one request with the next samples, in order, and silence where the
/// queue ran dry.
fn fill<T: SizedSample + FromSample<i16>>(data: &mut [T], samples: &Mutex<VecDeque<i16>>) {
    let Ok(mut queue) = samples.lock() else {
        data.fill(T::from_sample(0));
        return;
    };
    let mut taken = 0;
    for slot in data.iter_mut() {
        let sample = queue.get(taken).copied();
        taken += usize::from(sample.is_some());
        *slot = T::from_sample(sample.unwrap_or(0));
    }
    queue.drain(..taken);
}

fn record(faults: &Arc<Mutex<Vec<String>>>, error: &Error) {
    if let Ok(mut faults) = faults.lock() {
        faults.push(error.to_string());
    }
}

fn failed(message: impl std::fmt::Display) -> Error {
    Error::Audio(message.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Condvar;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{Queue, State, fill};

    #[test]
    fn samples_play_in_order_and_silence_follows_an_empty_queue() {
        let samples = Mutex::new(VecDeque::from(vec![1_i16, 2, 3, 4, 5, 6]));
        let mut data = [0_i16; 6];
        fill(&mut data, &samples);
        assert_eq!(data, [1, 2, 3, 4, 5, 6], "every sample plays in order");
        assert!(samples.lock().map_or(true, |queue| queue.is_empty()));
        fill(&mut data, &samples);
        assert_eq!(data, [0; 6], "an empty queue is silence");
    }

    #[test]
    fn the_queue_hands_out_its_shape_with_what_it_got() {
        let queue = Queue {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        };
        let stopped = AtomicBool::new(true);
        assert_eq!(
            queue.next(&stopped),
            None,
            "a stopped queue hands out nothing"
        );
        stopped.store(false, Ordering::Relaxed);
        queue.set_shape(2, 48_000);
        queue.push(&[7, 8]);
        assert_eq!(queue.next(&stopped), Some((Some((2, 48_000)), vec![7, 8])));
    }
}
