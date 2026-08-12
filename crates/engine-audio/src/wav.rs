//! A WAV reader, so a synthesised sound can be replaced by a recorded one.
//!
//! Deliberately small: RIFF/WAVE, PCM (8, 16, 24 and 32 bit) and IEEE float (32 and 64 bit),
//! mono or stereo, any sample rate. That is what every tool exports by default and what the
//! rest of the engine needs — nothing here is a general media pipeline, and compressed
//! formats are a decoder dependency this phase does not take (see `phase_audio.md`,
//! Milestone 2).
//!
//! # Everything is a recoverable error
//!
//! There is no panic path and no `unwrap`. A truncated file, a chunk that lies about its
//! length, an unknown format tag and a file that is not a WAV at all all return an
//! [`WavError`], and the caller keeps the sound it already had.
//!
//! # Stereo is folded to mono
//!
//! A [`SoundAsset`] is mono because the listener decides where a sound is. A stereo file has
//! already made that decision, so its channels are averaged on load and the fold is logged.

use std::fmt;
use std::fs;
use std::path::Path;

use crate::asset::SoundAsset;

/// Why a WAV file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    /// The file could not be opened or read.
    Io(String),
    /// It is not a RIFF/WAVE file.
    NotWave,
    /// It ended in the middle of something.
    Truncated,
    /// It has no `fmt ` chunk, or no `data` chunk.
    MissingChunk(&'static str),
    /// Its format tag is not PCM or IEEE float.
    UnsupportedFormat(u16),
    /// Its sample width is not one this reader handles.
    UnsupportedBits(u16),
    /// It has no channels, or more than two.
    UnsupportedChannels(u16),
}

impl fmt::Display for WavError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(formatter, "{message}"),
            Self::NotWave => write!(formatter, "not a RIFF/WAVE file"),
            Self::Truncated => write!(formatter, "the file ends mid-chunk"),
            Self::MissingChunk(name) => write!(formatter, "no '{name}' chunk"),
            Self::UnsupportedFormat(tag) => write!(formatter, "format tag {tag} is not PCM"),
            Self::UnsupportedBits(bits) => write!(formatter, "{bits}-bit samples"),
            Self::UnsupportedChannels(channels) => write!(formatter, "{channels} channels"),
        }
    }
}

impl std::error::Error for WavError {}

/// Reads `path` into a mono [`SoundAsset`].
///
/// # Errors
///
/// Returns [`WavError`] if the file cannot be read or is not a WAV this reader handles.
pub fn read(path: &Path) -> Result<SoundAsset, WavError> {
    let bytes = fs::read(path).map_err(|error| WavError::Io(error.to_string()))?;
    let asset = parse(&bytes)?;
    Ok(asset)
}

/// Parses the bytes of a WAV file into a mono [`SoundAsset`].
///
/// # Errors
///
/// Returns [`WavError`] if the bytes are not a WAV this reader handles.
pub fn parse(bytes: &[u8]) -> Result<SoundAsset, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }

    let mut format: Option<Format> = None;
    let mut data: Option<&[u8]> = None;
    let mut cursor = 12;

    // Walk the chunk list rather than assuming `fmt ` then `data`: real files carry `LIST`,
    // `fact` and padding chunks between them, and a reader that assumed an order would reject
    // most of what a recorder writes.
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(take4(bytes, cursor + 4)?) as usize;
        let start = cursor + 8;
        let end = start.checked_add(size).ok_or(WavError::Truncated)?;
        if end > bytes.len() {
            return Err(WavError::Truncated);
        }

        match id {
            b"fmt " => format = Some(Format::parse(&bytes[start..end])?),
            b"data" => data = Some(&bytes[start..end]),
            _ => {}
        }

        // Chunks are word-aligned: an odd size is followed by one pad byte that is not
        // counted in the size field.
        cursor = end + (size & 1);
    }

    let format = format.ok_or(WavError::MissingChunk("fmt "))?;
    let data = data.ok_or(WavError::MissingChunk("data"))?;
    let samples = format.decode(data)?;
    Ok(SoundAsset::new(samples, format.sample_rate))
}

/// What a `fmt ` chunk says.
#[derive(Debug, Clone, Copy)]
struct Format {
    tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
}

/// The format tag of integer PCM.
const FORMAT_PCM: u16 = 1;
/// The format tag of IEEE float.
const FORMAT_FLOAT: u16 = 3;
/// The format tag that says "look in the extension for the real one".
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

impl Format {
    /// Reads a `fmt ` chunk's body.
    fn parse(chunk: &[u8]) -> Result<Self, WavError> {
        if chunk.len() < 16 {
            return Err(WavError::Truncated);
        }
        let tag = u16::from_le_bytes([chunk[0], chunk[1]]);
        let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
        let sample_rate = u32::from_le_bytes(take4(chunk, 4)?);
        let bits = u16::from_le_bytes([chunk[14], chunk[15]]);

        // WAVE_FORMAT_EXTENSIBLE puts the real tag in the first two bytes of its GUID, which
        // is what a 24-bit or multichannel file from a modern recorder looks like.
        let tag = if tag == FORMAT_EXTENSIBLE && chunk.len() >= 26 {
            u16::from_le_bytes([chunk[24], chunk[25]])
        } else {
            tag
        };

        if tag != FORMAT_PCM && tag != FORMAT_FLOAT {
            return Err(WavError::UnsupportedFormat(tag));
        }
        if channels == 0 || channels > 2 {
            return Err(WavError::UnsupportedChannels(channels));
        }
        let supported = match tag {
            FORMAT_FLOAT => matches!(bits, 32 | 64),
            _ => matches!(bits, 8 | 16 | 24 | 32),
        };
        if !supported {
            return Err(WavError::UnsupportedBits(bits));
        }

        Ok(Self {
            tag,
            channels,
            sample_rate,
            bits,
        })
    }

    /// Turns a `data` chunk into mono samples in `[-1, 1]`.
    fn decode(self, data: &[u8]) -> Result<Vec<f32>, WavError> {
        let width = self.bits as usize / 8;
        let channels = self.channels as usize;
        let frame = width * channels;
        if frame == 0 {
            return Err(WavError::UnsupportedBits(self.bits));
        }
        let frames = data.len() / frame;
        let mut samples = Vec::with_capacity(frames);

        for index in 0..frames {
            let base = index * frame;
            let mut sum = 0.0_f32;
            for channel in 0..channels {
                let offset = base + channel * width;
                sum += self.sample(&data[offset..offset + width]);
            }
            samples.push(sum / channels as f32);
        }

        if channels == 2 {
            log::debug!("audio: a stereo file was folded to mono for positional playback");
        }
        Ok(samples)
    }

    /// One sample of `width` bytes, scaled into `[-1, 1]`.
    fn sample(self, bytes: &[u8]) -> f32 {
        match (self.tag, self.bits) {
            // 8-bit PCM is unsigned and centred on 128; every other integer width is signed.
            (FORMAT_PCM, 8) => (bytes[0] as f32 - 128.0) / 128.0,
            (FORMAT_PCM, 16) => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0,
            (FORMAT_PCM, 24) => {
                let value = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8;
                value as f32 / 8_388_608.0
            }
            (FORMAT_PCM, 32) => {
                i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32
                    / 2_147_483_648.0
            }
            (FORMAT_FLOAT, 32) => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            (FORMAT_FLOAT, 64) => f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]) as f32,
            // Unreachable: `Format::parse` rejects every other combination before this runs.
            _ => 0.0,
        }
    }
}

/// Four bytes at `offset`, or [`WavError::Truncated`].
fn take4(bytes: &[u8], offset: usize) -> Result<[u8; 4], WavError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(WavError::Truncated)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Builds a WAV file in memory: `bits`-wide `tag` samples at 8 kHz.
    fn wave(tag: u16, bits: u16, channels: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&8_000_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn sixteen_bit_mono_reads_at_the_right_level() {
        let mut data = Vec::new();
        for sample in [0_i16, 16_384, -16_384, 32_767] {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let asset = parse(&wave(FORMAT_PCM, 16, 1, &data)).expect("a valid file");
        assert_eq!(asset.sample_rate(), 8_000);
        assert_eq!(asset.len(), 4);
        assert!((asset.samples()[1] - 0.5).abs() < 1e-3);
        assert!((asset.samples()[2] + 0.5).abs() < 1e-3);
    }

    #[test]
    fn float_samples_come_through_unchanged() {
        let mut data = Vec::new();
        for sample in [0.25_f32, -0.75] {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let asset = parse(&wave(FORMAT_FLOAT, 32, 1, &data)).expect("a valid file");
        assert!((asset.samples()[0] - 0.25).abs() < 1e-6);
        assert!((asset.samples()[1] + 0.75).abs() < 1e-6);
    }

    #[test]
    fn stereo_is_folded_to_mono() {
        let mut data = Vec::new();
        for sample in [16_384_i16, -16_384, 32_767, 32_767] {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        let asset = parse(&wave(FORMAT_PCM, 16, 2, &data)).expect("a valid file");
        assert_eq!(asset.len(), 2, "two stereo frames are two mono samples");
        assert!(asset.samples()[0].abs() < 1e-3, "the pair cancels");
        assert!((asset.samples()[1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_chunk_between_fmt_and_data_is_skipped() {
        let mut bytes = wave(FORMAT_PCM, 16, 1, &0_i16.to_le_bytes());
        // Splice a LIST chunk in after `fmt `, which is where a recorder puts its metadata:
        // 12 bytes of RIFF header plus a 24-byte `fmt ` chunk.
        let insert = 36;
        let mut list = Vec::new();
        list.extend_from_slice(b"LIST");
        list.extend_from_slice(&5_u32.to_le_bytes());
        list.extend_from_slice(b"hello");
        list.push(0); // the pad byte an odd-sized chunk carries
        bytes.splice(insert..insert, list);
        assert!(parse(&bytes).is_ok());
    }

    #[test]
    fn malformed_files_are_errors_and_never_panics() {
        assert_eq!(parse(b"").expect_err("a malformed file"), WavError::NotWave);
        assert_eq!(
            parse(b"RIFF____NOPE").expect_err("a malformed file"),
            WavError::NotWave
        );

        let mut truncated = wave(FORMAT_PCM, 16, 1, &[0, 0, 0, 0]);
        truncated.truncate(truncated.len() - 2);
        assert_eq!(
            parse(&truncated).expect_err("a malformed file"),
            WavError::Truncated
        );

        assert_eq!(
            parse(&wave(7, 16, 1, &[0, 0])).expect_err("a malformed file"),
            WavError::UnsupportedFormat(7)
        );
        assert_eq!(
            parse(&wave(FORMAT_PCM, 12, 1, &[0, 0])).expect_err("a malformed file"),
            WavError::UnsupportedBits(12)
        );
        assert_eq!(
            parse(&wave(FORMAT_PCM, 16, 6, &[0, 0])).expect_err("a malformed file"),
            WavError::UnsupportedChannels(6)
        );

        // A `fmt ` chunk with no `data` chunk after it.
        let mut headers = wave(FORMAT_PCM, 16, 1, &[]);
        headers.truncate(headers.len() - 8);
        assert_eq!(
            parse(&headers).expect_err("a malformed file"),
            WavError::MissingChunk("data")
        );
    }

    #[test]
    fn an_empty_data_chunk_becomes_a_playable_silent_asset() {
        let asset = parse(&wave(FORMAT_PCM, 16, 1, &[])).expect("a valid, empty file");
        assert_eq!(asset.len(), 1);
    }

    #[test]
    fn a_missing_file_is_an_io_error_rather_than_a_panic() {
        let error =
            read(Path::new("no-such-directory/no-such-file.wav")).expect_err("a malformed file");
        assert!(matches!(error, WavError::Io(_)));
    }
}
