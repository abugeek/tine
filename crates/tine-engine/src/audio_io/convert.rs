//! Lock-free, allocation-free sample format conversion and channel downmix/upmix.
//!
//! All functions in this module are hard real-time safe.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use cpal::{Data, I24, Sample, SampleFormat, U24};

/// Converts an interleaved `cpal::Data` buffer into mono `f32` samples in `out`.
///
/// `frames_offset` is the starting frame index in `data`; at most `out.len()` frames are
/// converted. Returns the number of mono frames written to `out`.
#[allow(clippy::arithmetic_side_effects)] // Floating-point sample scaling/averaging.
pub(crate) fn downmix_to_mono(data: &Data, channels: usize, frames_offset: usize, out: &mut [f32]) -> usize {
    if channels == 0 || out.is_empty() {
        return 0;
    }

    match data.sample_format() {
        SampleFormat::F32 => downmix_slice(
            data.as_slice::<f32>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            |s| s,
        ),
        SampleFormat::F64 => downmix_slice(
            data.as_slice::<f64>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            |s| s as f32,
        ),
        SampleFormat::I16 => downmix_slice(
            data.as_slice::<i16>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::I24 => downmix_slice(
            data.as_slice::<I24>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::I32 => downmix_slice(
            data.as_slice::<i32>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::I64 => downmix_slice(
            data.as_slice::<i64>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::I8 => downmix_slice(
            data.as_slice::<i8>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::U8 => downmix_slice(
            data.as_slice::<u8>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::U16 => downmix_slice(
            data.as_slice::<u16>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::U24 => downmix_slice(
            data.as_slice::<U24>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::U32 => downmix_slice(
            data.as_slice::<u32>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        SampleFormat::U64 => downmix_slice(
            data.as_slice::<u64>().unwrap_or(&[]),
            channels,
            frames_offset,
            out,
            Sample::to_sample::<f32>,
        ),
        _ => {
            out.fill(0.0);
            0
        }
    }
}

#[allow(clippy::arithmetic_side_effects)] // Floating-point accumulation/division.
fn downmix_slice<T: Copy>(
    src: &[T],
    channels: usize,
    frames_offset: usize,
    out: &mut [f32],
    to_f32: impl Fn(T) -> f32,
) -> usize {
    let sample_offset = frames_offset.saturating_mul(channels);
    let Some(rest) = src.get(sample_offset..) else {
        return 0;
    };
    let inv_ch = 1.0_f32 / (channels as f32);
    let mut written = 0usize;

    for (frame, dst) in rest.chunks_exact(channels).zip(out.iter_mut()) {
        let mut sum = 0.0_f32;
        for &s in frame {
            let v = to_f32(s);
            sum += if v.is_finite() { v } else { 0.0 };
        }
        *dst = sum * inv_ch;
        written = written.saturating_add(1);
    }

    written
}

/// Upmixes mono `f32` samples in `mono` to all `channels` of `data`, converting to the
/// stream's [`SampleFormat`].
#[allow(clippy::arithmetic_side_effects)] // Floating-point clamping.
pub(crate) fn upmix_from_mono_at(mono: &[f32], channels: usize, data: &mut Data, frame_offset: usize) {
    if channels == 0 || mono.is_empty() {
        return;
    }

    match data.sample_format() {
        SampleFormat::F32 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<f32>().unwrap_or(&mut []),
            |s| s,
        ),
        SampleFormat::F64 => {
            upmix_slice(
                mono,
                channels,
                frame_offset,
                data.as_slice_mut::<f64>().unwrap_or(&mut []),
                f64::from,
            );
        }
        SampleFormat::I16 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<i16>().unwrap_or(&mut []),
            f32::to_sample::<i16>,
        ),
        SampleFormat::I24 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<I24>().unwrap_or(&mut []),
            f32::to_sample::<I24>,
        ),
        SampleFormat::I32 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<i32>().unwrap_or(&mut []),
            f32::to_sample::<i32>,
        ),
        SampleFormat::I64 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<i64>().unwrap_or(&mut []),
            f32::to_sample::<i64>,
        ),
        SampleFormat::I8 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<i8>().unwrap_or(&mut []),
            f32::to_sample::<i8>,
        ),
        SampleFormat::U8 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<u8>().unwrap_or(&mut []),
            f32::to_sample::<u8>,
        ),
        SampleFormat::U16 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<u16>().unwrap_or(&mut []),
            f32::to_sample::<u16>,
        ),
        SampleFormat::U24 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<U24>().unwrap_or(&mut []),
            f32::to_sample::<U24>,
        ),
        SampleFormat::U32 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<u32>().unwrap_or(&mut []),
            f32::to_sample::<u32>,
        ),
        SampleFormat::U64 => upmix_slice(
            mono,
            channels,
            frame_offset,
            data.as_slice_mut::<u64>().unwrap_or(&mut []),
            f32::to_sample::<u64>,
        ),
        _ => data.bytes_mut().fill(0),
    }
}

fn upmix_slice<T: Copy>(
    mono: &[f32],
    channels: usize,
    frame_offset: usize,
    dst: &mut [T],
    from_f32: impl Fn(f32) -> T,
) {
    let start = frame_offset.saturating_mul(channels);
    let Some(dst) = dst.get_mut(start..) else {
        return;
    };
    for (&m, frame) in mono.iter().zip(dst.chunks_exact_mut(channels)) {
        let clamped = if m.is_finite() { m.clamp(-1.0, 1.0) } else { 0.0 };
        let converted = from_f32(clamped);
        frame.fill(converted);
    }
}
