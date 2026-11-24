//! Reading the sound of any audio or video file through Windows Media
//! Foundation, as 16 kHz mono float: exactly what Whisper listens to. Windows
//! owns every decoder used here (MP4/AAC, MP3, FLAC, WMA, Opus in WebM and so
//! on), so the app ships no codecs of its own.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    IMFSourceReader, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION, MFAudioFormat_Float, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFSTARTUP_LITE, MFStartup,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Variant::VT_UI8;
use windows::core::HSTRING;

/// Whisper's sample rate.
pub const RATE: u32 = 16_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The file opened but has no sound (a silent screen recording).
    NoAudio,
    /// Windows has no reader for this file: unknown, damaged, or a format
    /// that needs a codec extension from the Store.
    Unreadable,
    Other(String),
}

impl From<windows::core::Error> for DecodeError {
    fn from(e: windows::core::Error) -> Self {
        DecodeError::Other(e.message())
    }
}

const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
/// MF_E_INVALIDSTREAMNUMBER: asking for the first audio stream of a file
/// that has none.
const INVALID_STREAM: u32 = 0xC00D_36B3;

fn start_media_foundation() -> Result<(), DecodeError> {
    unsafe {
        // Both are reference counted and safe to repeat on a thread.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
    }
    Ok(())
}

fn open_reader(source: &Path) -> Result<IMFSourceReader, DecodeError> {
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(source.as_os_str()), None)
            .map_err(|_| DecodeError::Unreadable)?;
        reader.SetStreamSelection(ALL_STREAMS, false)?;
        if let Err(e) = reader.SetStreamSelection(FIRST_AUDIO, true) {
            return Err(if e.code().0 as u32 == INVALID_STREAM {
                DecodeError::NoAudio
            } else {
                e.into()
            });
        }
        reader
            .GetNativeMediaType(FIRST_AUDIO, 0)
            .map_err(|_| DecodeError::NoAudio)?;
        Ok(reader)
    }
}

/// The source's length in seconds, when the file says.
fn duration(reader: &IMFSourceReader) -> Option<f64> {
    unsafe {
        let mut value = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()?;
        let inner = &value.Anonymous.Anonymous;
        let length = (inner.vt == VT_UI8).then(|| inner.Anonymous.uhVal);
        let _ = PropVariantClear(&mut value);
        length.filter(|d| *d > 0).map(|d| d as f64 / 10_000_000.0)
    }
}

/// Opens `source` just far enough to learn its length. `Err` when Windows
/// cannot read its sound.
pub fn probe(source: &Path) -> Result<Option<f64>, DecodeError> {
    start_media_foundation()?;
    let reader = open_reader(source)?;
    Ok(duration(&reader))
}

/// A file being read block by block.
pub struct Decoder {
    reader: IMFSourceReader,
    seconds: Option<f64>,
}

impl Decoder {
    pub fn open(source: &Path) -> Result<Self, DecodeError> {
        start_media_foundation()?;
        let reader = open_reader(source)?;
        let seconds = duration(&reader);
        unsafe {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)?;
            t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
            t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 1)?;
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 4)?;
            // The reader decodes, resamples and mixes down to exactly this.
            // Fails when Windows has no decoder for the track.
            reader
                .SetCurrentMediaType(FIRST_AUDIO, None, &t)
                .map_err(|_| DecodeError::Unreadable)?;
            reader.SetStreamSelection(FIRST_AUDIO, true)?;
        }
        Ok(Self { reader, seconds })
    }

    /// Length in seconds, when the file says.
    pub fn seconds(&self) -> Option<f64> {
        self.seconds
    }

    /// Appends the next block of sound to `out`. `false` at the end.
    pub fn read(&mut self, out: &mut Vec<f32>) -> Result<bool, DecodeError> {
        unsafe {
            loop {
                let mut flags = 0u32;
                let mut sample = None;
                self.reader
                    .ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), None, Some(&mut sample))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    return Ok(false);
                }
                // A stream tick or format change carries no data.
                let Some(sample) = sample else { continue };
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut data = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut data, None, Some(&mut len))?;
                let bytes = std::slice::from_raw_parts(data, len as usize);
                extend_from_le_floats(out, bytes);
                buffer.Unlock()?;
                return Ok(true);
            }
        }
    }
}

/// Appends little-endian 32-bit floats from `bytes`, ignoring a ragged end.
pub fn extend_from_le_floats(out: &mut Vec<f32>, bytes: &[u8]) {
    out.extend(
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a sound every Windows install has: it must come out at
    /// 16 kHz mono, as long as the file says it is.
    #[test]
    fn windows_sounds_decode_to_16_khz_mono() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let path = Path::new(&windir).join("Media").join("tada.wav");
        if !path.exists() {
            return;
        }
        let mut decoder = Decoder::open(&path).expect("Windows reads its own sounds");
        let seconds = decoder.seconds().expect("a WAV knows its length");
        let mut samples = Vec::new();
        while decoder.read(&mut samples).unwrap() {}
        let expected = seconds * RATE as f64;
        assert!((samples.len() as f64 - expected).abs() < RATE as f64 * 0.1, "{} vs {expected}", samples.len());
        assert!(samples.iter().any(|s| s.abs() > 0.01));
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.5));
        assert_eq!(probe(&path).unwrap().map(|s| (s * 10.0).round()), Some((seconds * 10.0).round()));
    }

    #[test]
    fn a_text_file_is_unreadable() {
        let path = std::env::temp_dir().join("audio-to-text-not-media.mp3");
        std::fs::write(&path, b"this is not audio").unwrap();
        assert!(Decoder::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn floats_are_read_little_endian_and_ragged_ends_dropped() {
        let mut out = vec![0.25];
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1.5f32.to_le_bytes());
        bytes.extend_from_slice(&(-0.5f32).to_le_bytes());
        bytes.push(7);
        extend_from_le_floats(&mut out, &bytes);
        assert_eq!(out, vec![0.25, 1.5, -0.5]);
    }
}
