mod image;
mod mask;

pub(crate) use image::{DecodedImage, decode_image};
pub(crate) use mask::{DecodedMask, decode_mask};

use crate::InterpreterWarning;
use crate::color::ColorSpace;
use crate::function::interpolate;
use crate::x_object::image::{ImageKind, ImageXObject};
use hayro_syntax::bit_reader::BitReader;
use hayro_syntax::object::Array;
use hayro_syntax::object::dict::keys::*;
use hayro_syntax::object::stream::{FilterResult, ImageColorSpace, ImageDecodeParams};
use smallvec::SmallVec;
use std::iter;

struct DecodeContext<'a> {
    decoded: FilterResult<'a>,
    width: u32,
    height: u32,
    scale_factors: (f32, f32),
    color_space: ColorSpace,
    bits_per_component: u8,
    decode_arr: SmallVec<[(f32, f32); 4]>,
}

fn decode_context<'a>(
    obj: &ImageXObject<'a>,
    target_dimension: Option<(u32, u32)>,
) -> Option<DecodeContext<'a>> {
    let dict = obj.stream.dict();
    let dict_bpc = dict
        .get::<u8>(BPC)
        .or_else(|| dict.get::<u8>(BITS_PER_COMPONENT));
    let color_space = obj.color_space.clone();
    let is_indexed = obj.color_space.as_ref().is_some_and(|cs| cs.is_indexed());

    let decode_params = ImageDecodeParams {
        is_indexed,
        bpc: dict_bpc,
        num_components: color_space.as_ref().map(|c| c.num_components()),
        target_dimension,
        width: obj.width,
        height: obj.height,
    };

    let mut decoded = obj
        .stream
        .decoded_image(&decode_params)
        .map_err(|_| (obj.warning_sink)(InterpreterWarning::ImageDecodeFailure))
        .ok()?;

    let (mut scale_x, mut scale_y) = (1.0_f32, 1.0_f32);

    let (width, height) = decoded
        .image_data
        .as_ref()
        .map(|d| {
            scale_x = obj.width as f32 / d.width as f32;
            scale_y = obj.height as f32 / d.height as f32;

            (d.width, d.height)
        })
        .unwrap_or((obj.width, obj.height));

    let color_space = color_space
        .or_else(|| {
            decoded
                .image_data
                .as_ref()
                .map(|i| i.color_space)
                .and_then(|c| {
                    c.and_then(|c| match c {
                        ImageColorSpace::Gray => Some(ColorSpace::device_gray()),
                        ImageColorSpace::Rgb => Some(ColorSpace::device_rgb()),
                        ImageColorSpace::Cmyk => Some(ColorSpace::device_cmyk()),
                        ImageColorSpace::Unknown(_) => None,
                    })
                })
        })
        .unwrap_or(ColorSpace::device_gray());

    let fallback_bpc = if obj.kind == ImageKind::StencilMask {
        1
    } else {
        8
    };

    let mut bits_per_component = decoded
        .image_data
        .as_ref()
        .map(|i| i.bits_per_component)
        .or(dict_bpc)
        .unwrap_or(fallback_bpc);

    // Katna PDF: an image drawn at a fraction of its size is averaged down
    // here, before it is expanded to a byte a sample and resampled
    // (vendor/hayro-interpret/KATNA.md).
    let (mut width, mut height) = (width, height);
    let factor = shrink_factor(width, height, target_dimension);
    if factor >= 2
        && !is_indexed
        && dict.get::<Array<'_>>(MASK).is_none()
        && let Some(shrunk) = average_down(
            &decoded.data,
            width,
            height,
            color_space.num_components() as usize,
            bits_per_component,
            factor,
        )
    {
        scale_x *= width as f32 / shrunk.width as f32;
        scale_y *= height as f32 / shrunk.height as f32;
        width = shrunk.width;
        height = shrunk.height;
        bits_per_component = 8;
        decoded.data = std::borrow::Cow::Owned(shrunk.data);
    }

    let decode_arr = dict
        .get::<Array<'_>>(D)
        .or_else(|| dict.get::<Array<'_>>(DECODE))
        .map(|a| a.iter::<(f32, f32)>().collect::<SmallVec<_>>())
        .unwrap_or(color_space.default_decode_arr(bits_per_component as f32));

    Some(DecodeContext {
        decoded,
        width,
        height,
        scale_factors: (scale_x, scale_y),
        color_space,
        bits_per_component,
        decode_arr,
    })
}

/// Images smaller than this many pixels are never averaged down: the
/// renderer's own resampling is fast enough for them.
const SHRINK_FROM_PIXELS: u64 = 1 << 22;

/// How many source pixels on a side one output pixel averages: the whole
/// number of times the image is larger than it will be drawn on both axes,
/// or 1 to keep it as it is.
fn shrink_factor(width: u32, height: u32, target: Option<(u32, u32)>) -> u32 {
    let Some((target_width, target_height)) = target else {
        return 1;
    };
    if u64::from(width) * u64::from(height) < SHRINK_FROM_PIXELS {
        return 1;
    }
    let x = width / target_width.max(1);
    let y = height / target_height.max(1);
    x.min(y).max(1)
}

struct Shrunk {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

/// Averages blocks of `factor`×`factor` samples of a `width`×`height`
/// image with `components` samples of `bpc` bits a pixel (rows padded to
/// whole bytes) into one 8-bit sample each. Edge blocks average what they
/// have. `None` for bit depths other than 1, 2, 4 and 8, or when the data
/// is too short (the usual decoding then handles it).
fn average_down(
    data: &[u8],
    width: u32,
    height: u32,
    components: usize,
    bpc: u8,
    factor: u32,
) -> Option<Shrunk> {
    if !matches!(bpc, 1 | 2 | 4 | 8) || components == 0 || factor < 2 {
        return None;
    }
    let (w, h, k) = (width as usize, height as usize, factor as usize);
    let bits = bpc as usize;
    let row_bytes = (w * components * bits).div_ceil(8);
    if data.len() < row_bytes.checked_mul(h)? {
        return None;
    }
    let (out_w, out_h) = (w.div_ceil(k), h.div_ceil(k));
    let max = (1u32 << bits) - 1;
    let mut sums = vec![0u64; out_w * components];
    let mut out = Vec::with_capacity(out_w * out_h * components);
    for band in 0..out_h {
        sums.fill(0);
        let rows = (band * k..((band + 1) * k).min(h)).map(|y| &data[y * row_bytes..][..row_bytes]);
        let band_height = rows.len();
        for row in rows {
            if bits == 1 && components == 1 {
                // One bit a pixel: count the set bits of each block.
                for (column, sum) in sums.iter_mut().enumerate() {
                    *sum += u64::from(count_ones(row, column * k, ((column + 1) * k).min(w)));
                }
            } else {
                for x in 0..w {
                    let column = x / k;
                    for c in 0..components {
                        sums[column * components + c] +=
                            u64::from(sample(row, x * components + c, bits));
                    }
                }
            }
        }
        for (column, chunk) in sums.chunks(components).enumerate() {
            let block_width = ((column + 1) * k).min(w) - column * k;
            let count = (block_width * band_height) as u64 * u64::from(max);
            for &sum in chunk {
                out.push(((sum * 255 + count / 2) / count) as u8);
            }
        }
    }
    Some(Shrunk {
        data: out,
        width: out_w as u32,
        height: out_h as u32,
    })
}

/// Sample `index` of a row of `bits`-bit samples, most significant first.
fn sample(row: &[u8], index: usize, bits: usize) -> u32 {
    if bits == 8 {
        return u32::from(row[index]);
    }
    let bit = index * bits;
    let byte = row[bit / 8];
    let shift = 8 - bits - bit % 8;
    u32::from((byte >> shift) & ((1u8 << bits) - 1))
}

/// The set bits of `row` from bit `start` up to `end`, most significant
/// bit first.
fn count_ones(row: &[u8], start: usize, end: usize) -> u32 {
    if start >= end {
        return 0;
    }
    let (first, last) = (start / 8, (end - 1) / 8);
    let head = 0xffu8 >> (start % 8);
    let tail = 0xffu8 << (7 - (end - 1) % 8);
    if first == last {
        return (row[first] & head & tail).count_ones();
    }
    let middle: u32 = row[first + 1..last].iter().map(|b| b.count_ones()).sum();
    (row[first] & head).count_ones() + middle + (row[last] & tail).count_ones()
}

#[must_use]
fn fix_image_length<T: Copy>(
    image: &mut Vec<T>,
    width: u32,
    height: &mut u32,
    filler: T,
    num_components: usize,
) -> Option<()> {
    let row_len = width as usize * num_components;

    if (row_len * *height as usize) <= image.len() {
        // Too much data (or just the right amount), truncate it.
        image.truncate(row_len * *height as usize);
    } else {
        // Too little data, adapt the height and pad.
        *height = image.len().div_ceil(row_len) as u32;

        if !image.len().is_multiple_of(row_len) {
            image.extend(iter::repeat_n(filler, row_len - (image.len() % row_len)));
        }
    }

    if width == 0 || *height == 0 {
        None
    } else {
        Some(())
    }
}

fn decode_u8_samples(
    data: &[u8],
    width: u32,
    height: u32,
    color_space: &ColorSpace,
    bits_per_component: u8,
    decode: &[(f32, f32)],
) -> Option<Vec<u8>> {
    let source_max = 2.0_f32.powi(bits_per_component as i32) - 1.0;
    let num_components = color_space.num_components() as usize;
    let capacity = width as usize * height as usize * num_components;
    let ranges = color_space.component_ranges();
    let indexed_hival = color_space.indexed_hival();

    let decode_component = |value: u32, index: usize| {
        let component_index = index % num_components;
        let (decode_min, decode_max) = *decode.get(component_index)?;
        let decoded = interpolate(value as f32, 0.0, source_max, decode_min, decode_max);

        if let Some(hival) = indexed_hival {
            Some((decoded + 0.5).clamp(0.0, hival as f32) as u8)
        } else {
            let (range_min, range_max) = *ranges.get(component_index)?;
            let normalized = if range_min == range_max {
                0.0
            } else {
                (decoded - range_min) / (range_max - range_min)
            };
            Some((normalized * 255.0 + 0.5) as u8)
        }
    };

    match bits_per_component {
        1..8 | 9..16 => {
            let mut buf = Vec::with_capacity(capacity);
            for_each_sample(
                data,
                width,
                height,
                num_components,
                bits_per_component,
                |value, index| {
                    buf.push(decode_component(value, index)?);
                    Some(())
                },
            )?;

            Some(buf)
        }
        8 => Some(
            data.iter()
                .enumerate()
                .map(|(index, value)| decode_component(*value as u32, index))
                .collect::<Option<Vec<_>>>()?,
        ),
        16 => Some(
            data.chunks_exact(2)
                .enumerate()
                .map(|(index, value)| {
                    decode_component(u16::from_be_bytes([value[0], value[1]]) as u32, index)
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => {
            warn!("unsupported bits per component: {bits_per_component}");
            None
        }
    }
}

fn unpack_samples(
    data: &[u8],
    width: u32,
    height: u32,
    num_components: usize,
    bits_per_component: u8,
) -> Option<Vec<u16>> {
    let capacity = width as usize * height as usize * num_components;

    match bits_per_component {
        1..8 | 9..16 => {
            let mut buf = Vec::with_capacity(capacity);
            for_each_sample(
                data,
                width,
                height,
                num_components,
                bits_per_component,
                |value, _| {
                    buf.push(value as u16);
                    Some(())
                },
            )?;

            Some(buf)
        }
        8 => Some(data.iter().map(|value| *value as u16).collect()),
        16 => Some(
            data.chunks_exact(2)
                .map(|value| u16::from_be_bytes([value[0], value[1]]))
                .collect(),
        ),
        _ => {
            warn!("unsupported bits per component: {bits_per_component}");
            None
        }
    }
}

fn for_each_sample(
    data: &[u8],
    width: u32,
    height: u32,
    num_components: usize,
    bits_per_component: u8,
    mut visit: impl FnMut(u32, usize) -> Option<()>,
) -> Option<()> {
    let mut reader = BitReader::new(data);
    let mut index = 0;

    for _ in 0..height {
        for _ in 0..width {
            for _ in 0..num_components {
                // See `stream_ccit_not_enough_data`, some images seemingly don't have
                // enough data, so we just pad with zeroes in this case.
                visit(reader.read(bits_per_component).unwrap_or(0), index)?;
                index += 1;
            }
        }

        reader.align();
    }

    Some(())
}
