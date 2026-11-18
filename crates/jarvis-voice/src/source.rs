use crate::error::{VoiceError, VoiceResult};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::{
    path::Path,
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Mutex,
    },
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// Samples were appended.
    Data,
    /// Nothing arrived within the timeout.
    Timeout,
    /// The source is finished (file ended or the stream closed).
    Ended,
}

/// A stream of mono f32 audio at its native sample rate.
pub trait AudioSource {
    fn device_name(&self) -> &str;
    fn sample_rate(&self) -> u32;
    /// Appends the next block of mono samples (-1..1) to `out`, waiting up to `timeout`.
    fn read(&mut self, out: &mut Vec<f32>, timeout: Duration) -> VoiceResult<ReadOutcome>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
}

fn device_error<E: std::fmt::Display>(error: E) -> VoiceError {
    VoiceError::Device(error.to_string())
}

fn name_of(device: &cpal::Device) -> String {
    device.description().map_or_else(
        |_| device.to_string(),
        |description| description.name().to_owned(),
    )
}

/// Input devices of the default audio host.
pub fn list_input_devices() -> VoiceResult<Vec<InputDevice>> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().map(|device| name_of(&device));
    let mut devices = Vec::new();
    for device in host.input_devices().map_err(device_error)? {
        let name = name_of(&device);
        devices.push(InputDevice {
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
        });
    }
    Ok(devices)
}

/// Microphone through cpal (WASAPI). Create and use it on one thread: the stream is not `Send`.
pub struct MicSource {
    // Keeps the capture running; dropped with the source.
    _stream: Stream,
    receiver: Receiver<Vec<f32>>,
    name: String,
    rate: u32,
    failure: Arc<Mutex<Option<String>>>,
}

trait ToF32: Copy {
    fn to_f32(self) -> f32;
}
impl ToF32 for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}
impl ToF32 for i16 {
    fn to_f32(self) -> f32 {
        f32::from(self) / 32768.0
    }
}
impl ToF32 for i32 {
    fn to_f32(self) -> f32 {
        self as f32 / 2_147_483_648.0
    }
}
impl ToF32 for u16 {
    fn to_f32(self) -> f32 {
        (f32::from(self) - 32768.0) / 32768.0
    }
}

fn downmix<T: ToF32>(data: &[T], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.iter().map(|sample| sample.to_f32()).collect();
    }
    data.chunks_exact(channels)
        .map(|frame| frame.iter().map(|sample| sample.to_f32()).sum::<f32>() / channels as f32)
        .collect()
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sender: mpsc::SyncSender<Vec<f32>>,
    failure: Arc<Mutex<Option<String>>>,
) -> VoiceResult<Stream>
where
    T: ToF32 + cpal::SizedSample + 'static,
{
    let channels = usize::from(config.channels);
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                // A full queue means the consumer is behind; dropping is better than blocking the driver.
                let _ = sender.try_send(downmix(data, channels));
            },
            move |error| {
                // An xrun is a glitch in the audio, not the end of the stream; the driver goes on.
                if matches!(
                    error.kind(),
                    cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied
                ) {
                    tracing::debug!(%error, "audio glitch ignored");
                    return;
                }
                if let Ok(mut slot) = failure.lock() {
                    *slot = Some(error.to_string());
                }
            },
            None,
        )
        .map_err(device_error)
}

impl MicSource {
    /// Opens the named device, or the system default when `name` is `None` or not found.
    pub fn open(name: Option<&str>) -> VoiceResult<Self> {
        let host = cpal::default_host();
        let mut chosen = None;
        if let Some(wanted) = name {
            for device in host.input_devices().map_err(device_error)? {
                if name_of(&device) == wanted {
                    chosen = Some(device);
                    break;
                }
            }
            if chosen.is_none() {
                tracing::warn!(device = wanted, "input device not found; using the default");
            }
        }
        let device = match chosen {
            Some(device) => device,
            None => host
                .default_input_device()
                .ok_or_else(|| VoiceError::Device("no input device".into()))?,
        };
        let device_name = name_of(&device);
        let supported = device.default_input_config().map_err(device_error)?;
        let format = supported.sample_format();
        let config = supported.config();
        let rate = config.sample_rate;
        let (sender, receiver) = mpsc::sync_channel(256);
        let failure = Arc::new(Mutex::new(None));
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, sender, Arc::clone(&failure))?,
            SampleFormat::I16 => build::<i16>(&device, config, sender, Arc::clone(&failure))?,
            SampleFormat::I32 => build::<i32>(&device, config, sender, Arc::clone(&failure))?,
            SampleFormat::U16 => build::<u16>(&device, config, sender, Arc::clone(&failure))?,
            other => {
                return Err(VoiceError::Device(format!(
                    "unsupported sample format {other:?}"
                )))
            }
        };
        stream.play().map_err(device_error)?;
        Ok(Self {
            _stream: stream,
            receiver,
            name: device_name,
            rate,
            failure,
        })
    }

    fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|slot| slot.clone())
    }
}

impl AudioSource for MicSource {
    fn device_name(&self) -> &str {
        &self.name
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn read(&mut self, out: &mut Vec<f32>, timeout: Duration) -> VoiceResult<ReadOutcome> {
        if let Some(message) = self.failure() {
            return Err(VoiceError::Device(message));
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(chunk) => {
                out.extend_from_slice(&chunk);
                while let Ok(more) = self.receiver.try_recv() {
                    out.extend_from_slice(&more);
                }
                Ok(ReadOutcome::Data)
            }
            Err(RecvTimeoutError::Timeout) => Ok(ReadOutcome::Timeout),
            Err(RecvTimeoutError::Disconnected) => Ok(ReadOutcome::Ended),
        }
    }
}

/// A WAV file as an audio source, delivered as fast as it is read (for tests and `--voice-wav`).
/// Silence is appended after the audio so that a final phrase can end.
pub struct WavSource {
    samples: Vec<f32>,
    position: usize,
    rate: u32,
    name: String,
    realtime: bool,
}

/// Block size per read: 10 ms.
const WAV_BLOCK_MS: usize = 10;

impl WavSource {
    /// Delivers audio at its natural speed (10 ms per 10 ms block) instead of as fast as possible.
    pub fn realtime(mut self, enabled: bool) -> Self {
        self.realtime = enabled;
        self
    }

    pub fn open(path: &Path, tail_silence_seconds: f32) -> VoiceResult<Self> {
        let file_error =
            |error: hound::Error| VoiceError::File(format!("{}: {error}", path.display()));
        let mut reader = hound::WavReader::open(path).map_err(file_error)?;
        let spec = reader.spec();
        let channels = usize::from(spec.channels.max(1));
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<Result<_, _>>()
                .map_err(file_error)?,
            hound::SampleFormat::Int => {
                let scale = (1_u64 << (spec.bits_per_sample.saturating_sub(1).min(31))) as f32;
                reader
                    .samples::<i32>()
                    .map(|sample| sample.map(|value| value as f32 / scale))
                    .collect::<Result<_, _>>()
                    .map_err(file_error)?
            }
        };
        let mut samples: Vec<f32> = interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect();
        let tail = (spec.sample_rate as f32 * tail_silence_seconds.max(0.0)) as usize;
        samples.extend(std::iter::repeat_n(0.0, tail));
        Ok(Self {
            samples,
            position: 0,
            rate: spec.sample_rate,
            name: path.display().to_string(),
            realtime: false,
        })
    }
}

impl AudioSource for WavSource {
    fn device_name(&self) -> &str {
        &self.name
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn read(&mut self, out: &mut Vec<f32>, _timeout: Duration) -> VoiceResult<ReadOutcome> {
        if self.position >= self.samples.len() {
            return Ok(ReadOutcome::Ended);
        }
        let block = (self.rate as usize * WAV_BLOCK_MS / 1000).max(1);
        let end = (self.position + block).min(self.samples.len());
        out.extend_from_slice(&self.samples[self.position..end]);
        self.position = end;
        if self.realtime {
            std::thread::sleep(Duration::from_millis(WAV_BLOCK_MS as u64));
        }
        Ok(ReadOutcome::Data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, channels: u16, rate: u32, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create");
        for sample in samples {
            writer.write_sample(*sample).expect("write");
        }
        writer.finalize().expect("finalize");
    }

    fn drain(source: &mut WavSource) -> Vec<f32> {
        let mut out = Vec::new();
        while source.read(&mut out, Duration::ZERO).expect("read") == ReadOutcome::Data {}
        out
    }

    #[test]
    fn wav_source_reads_mono_and_appends_silence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.wav");
        write_wav(&path, 1, 16_000, &[16384; 1600]);
        let mut source = WavSource::open(&path, 0.5).expect("open");
        assert_eq!(source.sample_rate(), 16_000);
        let samples = drain(&mut source);
        assert_eq!(samples.len(), 1600 + 8000);
        assert!((samples[0] - 0.5).abs() < 1e-4);
        assert_eq!(samples[1600], 0.0);
        assert_eq!(
            source.read(&mut Vec::new(), Duration::ZERO).expect("read"),
            ReadOutcome::Ended
        );
    }

    #[test]
    fn wav_source_downmixes_stereo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.wav");
        write_wav(&path, 2, 8_000, &[16384, -16384, 16384, 0]);
        let mut source = WavSource::open(&path, 0.0).expect("open");
        let samples = drain(&mut source);
        assert_eq!(samples.len(), 2);
        assert!(samples[0].abs() < 1e-4);
        assert!((samples[1] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn missing_wav_is_an_error() {
        let result = WavSource::open(Path::new("definitely-missing.wav"), 0.0);
        assert!(matches!(result, Err(VoiceError::File(_))));
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0_f32, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25_f32, 0.75], 1), vec![0.25, 0.75]);
    }
}
