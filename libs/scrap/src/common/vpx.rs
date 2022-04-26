#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(improper_ctypes)]
#![allow(dead_code)]

impl Default for vpx_codec_enc_cfg {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

impl Default for vpx_codec_ctx {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

impl Default for vpx_image_t {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

include!(concat!(env!("OUT_DIR"), "/vpx_ffi.rs"));

use super::codec::{
    Config, DecodeFrames, DecoderApi, EncodeFrame, EncodeFrames, EncoderApi, Error, Image,
    ImageApi, Result, VideoCodecId,
};
use std::os::raw::c_int;
use std::{ptr, slice};
use vp8e_enc_control_id::*;
use vpx_codec_err_t::*;

macro_rules! call_vpx {
    ($x:expr) => {{
        let result = unsafe { $x }; // original expression
        let result_int = unsafe { std::mem::transmute::<_, i32>(result) };
        if result_int != 0 {
            return Err(Error::FailedCall(format!(
                "errcode={} {}:{}:{}:{}",
                result_int,
                module_path!(),
                file!(),
                line!(),
                column!()
            ))
            .into());
        }
        result
    }};
}

macro_rules! call_vpx_ptr {
    ($x:expr) => {{
        let result = unsafe { $x }; // original expression
        let result_int = unsafe { std::mem::transmute::<_, isize>(result) };
        if result_int == 0 {
            return Err(Error::BadPtr(format!(
                "errcode={} {}:{}:{}:{}",
                result_int,
                module_path!(),
                file!(),
                line!(),
                column!()
            ))
            .into());
        }
        result
    }};
}

pub struct VpxEncoder {
    pub ctx: vpx_codec_ctx,
    pub encoder_type: VideoCodecId,
}

pub struct VpxEncodeFrames<'a> {
    pub ctx: &'a mut vpx_codec_ctx_t,
    pub iter: vpx_codec_iter_t,
}

impl<'a> Iterator for VpxEncodeFrames<'a> {
    type Item = EncodeFrame<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            unsafe {
                let pkt = vpx_codec_get_cx_data(self.ctx, &mut self.iter);
                if pkt.is_null() {
                    return None;
                } else if (*pkt).kind == vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT {
                    let f = &(*pkt).data.frame;
                    return Some(Self::Item {
                        data: slice::from_raw_parts(f.buf as _, f.sz as _),
                        key: (f.flags & VPX_FRAME_IS_KEY) != 0,
                        pts: f.pts,
                    });
                } else {
                    // Ignore the packet.
                }
            }
        }
    }
}

impl EncoderApi for VpxEncoder {
    fn new(config: &Config, num_threads: u32) -> Result<Self> {
        let i;
        if cfg!(feature = "VP8") || VideoCodecId::VP8 == config.codec {
            i = call_vpx_ptr!(vpx_codec_vp8_cx());
        } else {
            i = call_vpx_ptr!(vpx_codec_vp9_cx());
        }
        let mut c = unsafe { std::mem::MaybeUninit::zeroed().assume_init() };
        call_vpx!(vpx_codec_enc_config_default(i, &mut c, 0));

        // https://www.webmproject.org/docs/encoder-parameters/
        // default: c.rc_min_quantizer = 0, c.rc_max_quantizer = 63
        // try rc_resize_allowed later

        c.g_w = config.width;
        c.g_h = config.height;
        c.g_timebase.num = config.timebase[0];
        c.g_timebase.den = config.timebase[1];
        c.rc_target_bitrate = config.bitrate;
        c.rc_undershoot_pct = 95;
        c.rc_dropframe_thresh = 25;
        if config.rc_min_quantizer > 0 {
            c.rc_min_quantizer = config.rc_min_quantizer;
        }
        if config.rc_max_quantizer > 0 {
            c.rc_max_quantizer = config.rc_max_quantizer;
        }
        let mut speed = config.speed;
        if speed <= 0 {
            speed = 6;
        }

        c.g_threads = if num_threads == 0 {
            num_cpus::get() as _
        } else {
            num_threads
        };
        c.g_error_resilient = VPX_ERROR_RESILIENT_DEFAULT;
        // https://developers.google.com/media/vp9/bitrate-modes/
        // Constant Bitrate mode (CBR) is recommended for live streaming with VP9.
        c.rc_end_usage = vpx_rc_mode::VPX_CBR;
        // c.kf_min_dist = 0;
        // c.kf_max_dist = 999999;
        c.kf_mode = vpx_kf_mode::VPX_KF_DISABLED; // reduce bandwidth a lot

        /*
        VPX encoder支持two-pass encode，这是为了rate control的。
        对于两遍编码，就是需要整个编码过程做两次，第一次会得到一些新的控制参数来进行第二遍的编码，
        这样可以在相同的bitrate下得到最好的PSNR
        */

        let mut ctx = Default::default();
        call_vpx!(vpx_codec_enc_init_ver(
            &mut ctx,
            i,
            &c,
            0,
            VPX_ENCODER_ABI_VERSION as _
        ));

        if config.codec == VideoCodecId::VP9 {
            // set encoder internal speed settings
            // in ffmpeg, it is --speed option
            /*
            set to 0 or a positive value 1-16, the codec will try to adapt its
            complexity depending on the time it spends encoding. Increasing this
            number will make the speed go up and the quality go down.
            Negative values mean strict enforcement of this
            while positive values are adaptive
            */
            /* https://developers.google.com/media/vp9/live-encoding
            Speed 5 to 8 should be used for live / real-time encoding.
            Lower numbers (5 or 6) are higher quality but require more CPU power.
            Higher numbers (7 or 8) will be lower quality but more manageable for lower latency
            use cases and also for lower CPU power devices such as mobile.
            */
            call_vpx!(vpx_codec_control_(&mut ctx, VP8E_SET_CPUUSED as _, speed,));
            // set row level multi-threading
            /*
            as some people in comments and below have already commented,
            more recent versions of libvpx support -row-mt 1 to enable tile row
            multi-threading. This can increase the number of tiles by up to 4x in VP9
            (since the max number of tile rows is 4, regardless of video height).
            To enable this, use -tile-rows N where N is the number of tile rows in
            log2 units (so -tile-rows 1 means 2 tile rows and -tile-rows 2 means 4 tile
            rows). The total number of active threads will then be equal to
            $tile_rows * $tile_columns
            */
            call_vpx!(vpx_codec_control_(
                &mut ctx,
                VP9E_SET_ROW_MT as _,
                1 as c_int
            ));

            call_vpx!(vpx_codec_control_(
                &mut ctx,
                VP9E_SET_TILE_COLUMNS as _,
                4 as c_int
            ));
        }

        return Ok(Self {
            ctx,
            encoder_type: config.codec,
        });
    }

    fn encode(
        &mut self,
        pts: i64,
        data: &[u8],
        stride_align: usize,
        width: usize,
        height: usize,
    ) -> Result<EncodeFrames> {
        assert!(2 * data.len() >= 3 * width * height);

        let mut image = Default::default();
        call_vpx_ptr!(vpx_img_wrap(
            &mut image,
            vpx_img_fmt::VPX_IMG_FMT_I420,
            width as _,
            height as _,
            stride_align as _,
            data.as_ptr() as _,
        ));

        call_vpx!(vpx_codec_encode(
            &mut self.ctx,
            &image,
            pts as _,
            1, // Duration
            0, // Flags
            VPX_DL_REALTIME as _,
        ));

        Ok(EncodeFrames {
            vpx_frame: Some(VpxEncodeFrames {
                ctx: &mut self.ctx,
                iter: ptr::null(),
            }),
            aom_frame: None,
            frame_type: self.encoder_type,
        })
    }

    fn flush(&mut self) -> Result<EncodeFrames> {
        call_vpx!(vpx_codec_encode(
            &mut self.ctx,
            ptr::null(),
            -1, // PTS
            1,  // Duration
            0,  // Flags
            VPX_DL_REALTIME as _,
        ));

        Ok(EncodeFrames {
            vpx_frame: Some(VpxEncodeFrames {
                ctx: &mut self.ctx,
                iter: ptr::null(),
            }),
            aom_frame: None,
            frame_type: self.encoder_type,
        })
    }
}

impl Drop for VpxEncoder {
    fn drop(&mut self) {
        unsafe {
            let result = vpx_codec_destroy(&mut self.ctx);
            if result != VPX_CODEC_OK {
                panic!("failed to destroy vpx codec");
            }
        }
    }
}

pub struct VpxDecodeFrames<'a> {
    ctx: &'a mut vpx_codec_ctx_t,
    iter: vpx_codec_iter_t,
}

impl<'a> Iterator for VpxDecodeFrames<'a> {
    type Item = Image<VpxImage>;
    fn next(&mut self) -> Option<Self::Item> {
        let img = unsafe { vpx_codec_get_frame(self.ctx, &mut self.iter) };
        if img.is_null() {
            return None;
        } else {
            return Some(Image::new(VpxImage(img)));
        }
    }
}

pub struct VpxDecoder {
    ctx: vpx_codec_ctx_t,
    decoder_type: VideoCodecId,
}

impl DecoderApi for VpxDecoder {
    /// Create a new decoder
    ///
    /// # Errors
    ///
    /// The function may fail if the underlying libvpx does not provide
    /// the VP9 decoder.
    fn new(codec: VideoCodecId, num_threads: u32) -> Result<Self> {
        // This is sound because `vpx_codec_ctx` is a repr(C) struct without any field that can
        // cause UB if uninitialized.
        let i;
        if cfg!(feature = "VP8") {
            i = match codec {
                VideoCodecId::VP8 => call_vpx_ptr!(vpx_codec_vp8_dx()),
                VideoCodecId::VP9 => call_vpx_ptr!(vpx_codec_vp9_dx()),
                VideoCodecId::AV1 => {
                    panic!("Init vpx encoder failed");
                }
            };
        } else {
            i = call_vpx_ptr!(vpx_codec_vp9_dx());
        }
        let mut ctx = Default::default();
        let cfg = vpx_codec_dec_cfg_t {
            threads: if num_threads == 0 {
                num_cpus::get() as _
            } else {
                num_threads
            },
            w: 0,
            h: 0,
        };
        /*
        unsafe {
            println!("{}", vpx_codec_get_caps(i));
        }
        */
        call_vpx!(vpx_codec_dec_init_ver(
            &mut ctx,
            i,
            &cfg,
            0,
            VPX_DECODER_ABI_VERSION as _,
        ));
        Ok(Self {
            ctx,
            decoder_type: codec,
        })
    }

    fn decode2rgb(&mut self, data: &[u8], rgba: bool) -> Result<Vec<u8>> {
        let img = Image::new(VpxImage::new());
        // for frame in self.decode(data)? {
        //     drop(img);
        //     img = frame;
        // }
        // for frame in self.flush()? {
        //     drop(img);
        //     img = frame;
        // }
        // if img.is_null() {
        //     Ok(Vec::new())
        // } else {
        let mut out = Default::default();
        img.rgb(1, rgba, &mut out);
        Ok(out)
        // }
    }

    /// Feed some compressed data to the encoder
    ///
    /// The `data` slice is sent to the decoder
    ///
    /// It matches a call to `vpx_codec_decode`.
    fn decode(&mut self, data: &[u8]) -> Result<DecodeFrames> {
        call_vpx!(vpx_codec_decode(
            &mut self.ctx,
            data.as_ptr(),
            data.len() as _,
            ptr::null_mut(),
            0,
        ));

        Ok(DecodeFrames {
            vpx_frame: Some(VpxDecodeFrames {
                ctx: &mut self.ctx,
                iter: ptr::null(),
            }),
            aom_frame: None,
            frame_type: self.decoder_type,
        })
    }

    fn flush(&mut self) -> Result<DecodeFrames> {
        call_vpx!(vpx_codec_decode(
            &mut self.ctx,
            ptr::null(),
            0,
            ptr::null_mut(),
            0
        ));
        Ok(DecodeFrames {
            vpx_frame: Some(VpxDecodeFrames {
                ctx: &mut self.ctx,
                iter: ptr::null(),
            }),
            aom_frame: None,
            frame_type: self.decoder_type,
        })
    }
}

impl Drop for VpxDecoder {
    fn drop(&mut self) {
        unsafe {
            let result = vpx_codec_destroy(&mut self.ctx);
            if result != VPX_CODEC_OK {
                panic!("failed to destroy vpx codec");
            }
        }
    }
}

pub struct VpxImage(*mut vpx_image_t);

impl VpxImage {
    #[inline]
    fn inner(&self) -> &vpx_image_t {
        unsafe { &*self.0 }
    }
}

impl ImageApi for VpxImage {
    #[inline]
    fn new() -> Self {
        Self(std::ptr::null_mut())
    }

    #[inline]
    fn is_null(&self) -> bool {
        self.0.is_null()
    }

    #[inline]
    fn width(&self) -> usize {
        self.inner().d_w as _
    }

    #[inline]
    fn height(&self) -> usize {
        self.inner().d_h as _
    }

    #[inline]
    fn stride(&self, iplane: usize) -> i32 {
        self.inner().stride[iplane]
    }

    fn rgb(&self, stride_align: usize, rgba: bool, dst: &mut Vec<u8>) {
        let h = self.height();
        let mut w = self.width();
        let bps = if rgba { 4 } else { 3 };
        w = (w + stride_align - 1) & !(stride_align - 1);
        dst.resize(h * w * bps, 0);
        let img = self.inner();
        unsafe {
            if rgba {
                super::I420ToARGB(
                    img.planes[0],
                    img.stride[0],
                    img.planes[1],
                    img.stride[1],
                    img.planes[2],
                    img.stride[2],
                    dst.as_mut_ptr(),
                    (w * bps) as _,
                    self.width() as _,
                    self.height() as _,
                );
            } else {
                super::I420ToRAW(
                    img.planes[0],
                    img.stride[0],
                    img.planes[1],
                    img.stride[1],
                    img.planes[2],
                    img.stride[2],
                    dst.as_mut_ptr(),
                    (w * bps) as _,
                    self.width() as _,
                    self.height() as _,
                );
            }
        }
    }

    #[inline]
    fn data(&self) -> (&[u8], &[u8], &[u8]) {
        unsafe {
            let img = self.inner();
            let h = (img.d_h as usize + 1) & !1;
            let n = img.stride[0] as usize * h;
            let y = slice::from_raw_parts(img.planes[0], n);
            let n = img.stride[1] as usize * (h >> 1);
            let u = slice::from_raw_parts(img.planes[1], n);
            let v = slice::from_raw_parts(img.planes[2], n);
            (y, u, v)
        }
    }
}

impl Drop for VpxImage {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { vpx_img_free(self.0) };
        }
    }
}

unsafe impl Send for vpx_codec_ctx_t {}
