#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(improper_ctypes)]
#![allow(dead_code)]

impl Default for aom_codec_enc_cfg {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

impl Default for aom_codec_ctx {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

impl Default for aom_image {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

include!(concat!(env!("OUT_DIR"), "/aom_ffi.rs"));

use super::codec::{
    Config, DecodeFrames, DecoderApi, EncodeFrame, EncodeFrames, EncoderApi, Error, Image,
    ImageApi, Result, VideoCodecId,
};
use std::{ptr, slice};

macro_rules! call_aom {
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

macro_rules! call_aom_ptr {
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

pub struct AomEncoder {
    pub ctx: aom_codec_ctx,
}

impl AomEncoder {
    fn get_cpu_speed(width: u32, height: u32, number_of_cores: u32) -> u32 {
        if number_of_cores > 2 && width * height <= 320 * 180 {
            return 6;
        } else if width * height >= 1280 * 720 {
            return 8;
        }
        7
    }
}

pub struct AomEncodeFrames<'a> {
    pub ctx: &'a mut aom_codec_ctx_t,
    pub iter: aom_codec_iter_t,
}

impl<'a> Iterator for AomEncodeFrames<'a> {
    type Item = EncodeFrame<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            unsafe {
                let pkt = aom_codec_get_cx_data(self.ctx, &mut self.iter);
                if pkt.is_null() {
                    return None;
                } else if (*pkt).kind == aom_codec_cx_pkt_kind_AOM_CODEC_CX_FRAME_PKT {
                    let f = &(*pkt).data.frame;
                    return Some(Self::Item {
                        data: slice::from_raw_parts(f.buf as _, f.sz as _),
                        key: (f.flags & AOM_FRAME_IS_KEY) != 0,
                        pts: f.pts,
                    });
                } else {
                    // Ignore the packet.
                }
            }
        }
    }
}

impl EncoderApi for AomEncoder {
    fn new(config: &Config, num_threads: u32) -> Result<Self> {
        let cx = call_aom_ptr!(aom_codec_av1_cx());
        let mut cfg = unsafe { std::mem::MaybeUninit::zeroed().assume_init() };
        call_aom!(aom_codec_enc_config_default(cx, &mut cfg, 0));

        cfg.g_w = config.width;
        cfg.g_h = config.height;
        cfg.g_threads = num_threads;
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = 90000;
        cfg.rc_target_bitrate = config.bitrate; // kilobits/sec.
        cfg.g_input_bit_depth = 8;
        cfg.kf_mode = aom_kf_mode_AOM_KF_DISABLED;
        if config.rc_min_quantizer > 0 {
            cfg.rc_min_quantizer = config.rc_min_quantizer;
        }
        if config.rc_max_quantizer > 0 {
            cfg.rc_max_quantizer = config.rc_max_quantizer;
        }
        cfg.g_usage = 1; // 0 = good quality; 1 = real-time.
        cfg.g_error_resilient = 0;
        // Low-latency settings.
        cfg.rc_end_usage = aom_rc_mode_AOM_CBR; // Constant Bit Rate (CBR) mode
        cfg.g_pass = aom_enc_pass_AOM_RC_ONE_PASS; // One-pass rate control
        cfg.g_lag_in_frames = 0; // No look ahead when lag equals 0.

        let mut ctx = Default::default();
        call_aom!(aom_codec_enc_init_ver(
            &mut ctx,
            cx,
            &cfg,
            0,
            AOM_ENCODER_ABI_VERSION as _
        ));

        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AOME_SET_CPUUSED as _,
            Self::get_cpu_speed(cfg.g_w, cfg.g_h, num_threads)
        ));

        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_ENABLE_CDEF as _,
            1
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_ENABLE_TPL_MODEL as _,
            0
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_DELTAQ_MODE as _,
            0
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_AQ_MODE as _,
            3
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AOME_SET_MAX_INTRA_BITRATE_PCT as _,
            300
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_COEFF_COST_UPD_FREQ as _,
            2
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_MODE_COST_UPD_FREQ as _,
            2
        ));
        call_aom!(aom_codec_control(
            &mut ctx,
            aome_enc_control_id_AV1E_SET_MV_COST_UPD_FREQ as _,
            3
        ));

        return Ok(Self { ctx });
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
        call_aom_ptr!(aom_img_wrap(
            &mut image,
            aom_img_fmt_AOM_IMG_FMT_I420,
            width as _,
            height as _,
            stride_align as _,
            data.as_ptr() as _,
        ));

        call_aom!(aom_codec_encode(
            &mut self.ctx,
            &image,
            pts as _,
            1, // Duration
            0, // Flags
        ));

        Ok(EncodeFrames {
            aom_frame: Some(AomEncodeFrames {
                ctx: &mut self.ctx,
                iter: ptr::null(),
            }),
            vpx_frame: None,
            frame_type: VideoCodecId::AV1,
        })
    }

    fn flush(&mut self) -> Result<EncodeFrames> {
        call_aom!(aom_codec_encode(
            &mut self.ctx,
            ptr::null(),
            -1, // PTS
            1,  // Duration
            0,  // Flags
        ));

        Ok(EncodeFrames {
            aom_frame: Some(AomEncodeFrames {
                ctx: &mut self.ctx,
                iter: ptr::null(),
            }),
            vpx_frame: None,
            frame_type: VideoCodecId::AV1,
        })
    }
}

impl Drop for AomEncoder {
    fn drop(&mut self) {
        unsafe {
            let result = aom_codec_destroy(&mut self.ctx);
            if result != aom_codec_err_t_AOM_CODEC_OK {
                panic!("failed to destroy aom codec");
            }
        }
    }
}
