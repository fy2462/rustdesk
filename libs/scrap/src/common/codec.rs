// https://github.com/astraw/vpx-encode
// https://github.com/astraw/env-libvpx-sys
// https://github.com/rust-av/vpx-rs/blob/master/src/decoder.rs

use super::aom::*;
use super::vpx::*;
use std::os::raw::{c_int, c_uint};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum VideoCodecId {
    VP8,
    VP9,
    AV1,
}

impl Default for VideoCodecId {
    fn default() -> VideoCodecId {
        VideoCodecId::VP9
    }
}

pub trait EncoderApi {
    fn new(config: &Config, num_threads: u32) -> Result<Self>
    where
        Self: Sized;
    fn encode(
        &mut self,
        pts: i64,
        data: &[u8],
        stride_align: usize,
        width: usize,
        height: usize,
    ) -> Result<EncodeFrames>;
    fn flush(&mut self) -> Result<EncodeFrames>;
}

pub struct Encoder {
    inner: Box<dyn EncoderApi>,
    width: usize,
    height: usize,
}

#[derive(Debug)]
pub enum Error {
    FailedCall(String),
    BadPtr(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

impl Encoder {
    pub fn new(config: &Config, num_threads: u32) -> Result<Self> {
        let encoder = match config.codec {
            VideoCodecId::VP8 | VideoCodecId::VP9 => {
                Box::new(VpxEncoder::new(config, num_threads).ok().unwrap())
            }
            VideoCodecId::AV1 => Box::new(AomEncoder::new(config, num_threads).ok().unwrap()),
        };
        Ok(Self {
            inner: encoder,
            width: config.width as _,
            height: config.height as _,
        })
    }

    pub fn encode(&mut self, pts: i64, data: &[u8], stride_align: usize) -> Result<EncodeFrames> {
        self.inner
            .encode(pts, data, stride_align, self.width, self.height)
    }

    /// Notify the encoder to return any pending packets
    pub fn flush(&mut self) -> Result<EncodeFrames> {
        self.inner.flush()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EncodeFrame<'a> {
    /// Compressed data.
    pub data: &'a [u8],
    /// Whether the frame is a keyframe.
    pub key: bool,
    /// Presentation timestamp (in timebase units).
    pub pts: i64,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// The width (in pixels).
    pub width: c_uint,
    /// The height (in pixels).
    pub height: c_uint,
    /// The timebase numerator and denominator (in seconds).
    pub timebase: [c_int; 2],
    /// The target bitrate (in kilobits per second).
    pub bitrate: c_uint,
    /// The codec
    pub codec: VideoCodecId,
    pub rc_min_quantizer: u32,
    pub rc_max_quantizer: u32,
    pub speed: i32,
}

#[derive(Default)]
pub struct EncodeFrames<'a> {
    pub vpx_frame: Option<VpxEncodeFrames<'a>>,
    pub aom_frame: Option<AomEncodeFrames<'a>>,
    pub frame_type: VideoCodecId,
}

impl<'a> Iterator for EncodeFrames<'a> {
    type Item = EncodeFrame<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let item = match self.frame_type {
            VideoCodecId::VP8 | VideoCodecId::VP9 => self.vpx_frame.unwrap().next(),
            VideoCodecId::AV1 => self.aom_frame.unwrap().next(),
        };
        return item;
    }
}

pub trait DecoderApi {
    fn new(codec: VideoCodecId, num_threads: u32) -> Result<Self>
    where
        Self: Sized;
    fn decode2rgb(&mut self, data: &[u8], rgba: bool) -> Result<Vec<u8>>;
    fn decode(&mut self, data: &[u8]) -> Result<DecodeFrames>;
    fn flush(&mut self) -> Result<DecodeFrames>;
}

pub struct Decoder {
    inner: Box<dyn DecoderApi>,
}

impl Decoder {
    /// Create a new decoder
    ///
    /// # Errors
    ///
    /// The function may fail if the underlying libvpx does not provide
    /// the VP9 decoder.
    pub fn new(codec: VideoCodecId, num_threads: u32) -> Result<Self> {
        let decoder = match codec {
            VideoCodecId::VP8 | VideoCodecId::VP9 => {
                VpxDecoder::new(codec, num_threads).ok().unwrap()
            }
            VideoCodecId::AV1 => VpxDecoder::new(codec, num_threads).ok().unwrap(),
        };
        Ok(Self {
            inner: Box::new(decoder),
        })
    }

    pub fn decode2rgb(&mut self, data: &[u8], rgba: bool) -> Result<Vec<u8>> {
        self.inner.decode2rgb(data, rgba)
    }

    pub fn decode(&mut self, data: &[u8]) -> Result<DecodeFrames> {
        self.inner.decode(data)
    }

    /// Notify the decoder to return any pending frame
    pub fn flush(&mut self) -> Result<DecodeFrames> {
        self.inner.flush()
    }
}

pub struct DecodeFrames<'a> {
    pub vpx_frame: Option<VpxDecodeFrames<'a>>,
    pub frame_type: VideoCodecId,
}

impl<'a> Iterator for DecodeFrames<'a> {
    type Item = Image<VpxImage>;
    fn next(&mut self) -> Option<Self::Item> {
        let item = match self.frame_type {
            VideoCodecId::VP8 | VideoCodecId::VP9 => self.vpx_frame.unwrap().next(),
            VideoCodecId::AV1 => self.vpx_frame.unwrap().next(),
        };
        return item;
    }
}

// https://chromium.googlesource.com/webm/libvpx/+/bali/vpx/src/vpx_image.c

pub trait ImageApi {
    fn new() -> Self;
    fn is_null(&self) -> bool;
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn stride(&self, iplane: usize) -> i32;
    fn data(&self) -> (&[u8], &[u8], &[u8]);
    fn rgb(&self, stride_align: usize, rgba: bool, dst: &mut Vec<u8>);
}

pub struct Image<T: ImageApi> {
    inner: T,
}

impl<T: ImageApi> Image<T> {
    #[inline]
    pub fn new(inner: T) -> Self {
        Image { inner }
    }

    #[inline]
    pub fn is_null(&self) -> bool {
        self.inner.is_null()
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.inner.width()
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.inner.height()
    }

    #[inline]
    pub fn stride(&self, iplane: usize) -> i32 {
        self.inner.stride(iplane)
    }

    pub fn rgb(&self, stride_align: usize, rgba: bool, dst: &mut Vec<u8>) {
        self.inner.rgb(stride_align, rgba, dst)
    }

    #[inline]
    pub fn data(&self) -> (&[u8], &[u8], &[u8]) {
        self.inner.data()
    }
}
