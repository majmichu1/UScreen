//! In-process H.264 encoding through libavcodec.
//!
//! The default path pipes raw NV12 into an `ffmpeg` child process. That works,
//! but ffmpeg reads its input in ~32KB chunks (its `-blocksize` option applies
//! to output only), so an 8.2MB frame costs roughly 250 read syscalls — about
//! 12,500 a second at 50fps, which is the likeliest source of the ~337% CPU the
//! encoder process burns under load.
//!
//! Encoding here removes that process boundary, and with it the ability to ask
//! for a keyframe on demand stops being impossible: the CLI has no way to force
//! one mid-stream, which is why a client reconnecting to an idle screen can wait
//! seconds for a picture.
//!
//! Built only with the `inproc-encoder` feature; see host/Cargo.toml for why.

use anyhow::{Context, Result};
use bytes::Bytes;

pub struct Encoder {
    inner: ffmpeg_next::encoder::Video,
    frame: ffmpeg_next::frame::Video,
    pts: i64,
}

impl Encoder {
    /// `name` is an libavcodec encoder name such as `h264_nvenc`.
    pub fn new(
        name: &str,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: u32,
        quality: u32,
    ) -> Result<Self> {
        ffmpeg_next::init().context("initialise libavcodec")?;

        let codec = ffmpeg_next::encoder::find_by_name(name)
            .with_context(|| format!("encoder {} not available in this libavcodec", name))?;

        let mut ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .context("open video encoder")?;

        ctx.set_width(width);
        ctx.set_height(height);
        ctx.set_format(ffmpeg_next::format::Pixel::NV12);
        ctx.set_time_base(ffmpeg_next::Rational(1, fps.max(1) as i32));
        ctx.set_frame_rate(Some(ffmpeg_next::Rational(fps.max(1) as i32, 1)));
        // One second between keyframes. Long, because with an on-demand IDR
        // available there is no reason to spend bits on periodic ones.
        ctx.set_gop(fps.max(1));
        ctx.set_max_b_frames(0);
        ctx.set_bit_rate(0);
        ctx.set_max_bit_rate((bitrate_kbps as usize) * 1000);
        ctx.set_colorspace(ffmpeg_next::color::Space::BT709);
        ctx.set_color_range(ffmpeg_next::color::Range::MPEG);

        let mut opts = ffmpeg_next::Dictionary::new();
        Self::low_latency_options(name, quality, bitrate_kbps, fps, &mut opts);

        let inner = ctx
            .open_with(opts)
            .with_context(|| format!("configure {}", name))?;

        let mut frame = ffmpeg_next::frame::Video::new(
            ffmpeg_next::format::Pixel::NV12,
            width,
            height,
        );
        frame.set_color_range(ffmpeg_next::color::Range::MPEG);

        Ok(Self { inner, frame, pts: 0 })
    }

    /// Encoder-specific knobs. Kept in one place so the CLI path and this one
    /// cannot drift apart in what they actually ask the hardware for.
    fn low_latency_options(
        name: &str,
        quality: u32,
        bitrate_kbps: u32,
        fps: u32,
        opts: &mut ffmpeg_next::Dictionary,
    ) {
        let bufsize = ((bitrate_kbps / fps.max(1)).max(200) * 1000).to_string();
        if name.contains("nvenc") {
            for (k, v) in [
                ("preset", "p1"),
                ("tune", "ull"),
                ("zerolatency", "1"),
                ("delay", "0"),
                ("rc", "vbr"),
                ("multipass", "0"),
                ("rc-lookahead", "0"),
                ("forced-idr", "1"),
            ] {
                opts.set(k, v);
            }
            opts.set("cq", &quality.to_string());
            opts.set("bufsize", &bufsize);
        } else if name.contains("vaapi") {
            opts.set("rc_mode", "CQP");
            opts.set("qp", &quality.to_string());
        } else {
            opts.set("preset", "ultrafast");
            opts.set("tune", "zerolatency");
            opts.set("crf", &quality.to_string());
        }
    }

    /// Feed one packed NV12 frame and collect whatever access units come out.
    ///
    /// `force_idr` makes the next frame a keyframe, which is what lets a client
    /// that just connected start decoding immediately instead of waiting for
    /// the next scheduled one.
    pub fn encode(&mut self, nv12: &[u8], force_idr: bool) -> Result<Vec<(Bytes, bool)>> {
        let (w, h) = (self.frame.width() as usize, self.frame.height() as usize);
        if nv12.len() < w * h * 3 / 2 {
            anyhow::bail!("short NV12 frame: {} bytes for {}x{}", nv12.len(), w, h);
        }

        // libavcodec frames are stride-padded; the helper packs tightly, so
        // copy row by row rather than assuming the two agree. Strides are read
        // first: taking them while a mutable borrow of the plane is live would
        // not pass the borrow checker.
        let (y_stride, uv_stride) = (self.frame.stride(0), self.frame.stride(1));
        copy_plane(self.frame.data_mut(0), y_stride, &nv12[..w * h], w, h);
        copy_plane(
            self.frame.data_mut(1),
            uv_stride,
            &nv12[w * h..w * h * 3 / 2],
            w,
            h / 2,
        );

        self.frame.set_pts(Some(self.pts));
        self.pts += 1;
        if force_idr {
            self.frame.set_kind(ffmpeg_next::picture::Type::I);
        } else {
            self.frame.set_kind(ffmpeg_next::picture::Type::None);
        }

        self.inner.send_frame(&self.frame).context("send frame")?;
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<(Bytes, bool)>> {
        let mut out = Vec::new();
        let mut packet = ffmpeg_next::Packet::empty();
        while self.inner.receive_packet(&mut packet).is_ok() {
            if let Some(data) = packet.data() {
                let is_idr = packet.is_key();
                out.push((Bytes::copy_from_slice(data), is_idr));
            }
        }
        Ok(out)
    }
}

fn copy_plane(dst: &mut [u8], stride: usize, src: &[u8], row_bytes: usize, rows: usize) {
    for y in 0..rows {
        let d = y * stride;
        let s = y * row_bytes;
        if d + row_bytes <= dst.len() && s + row_bytes <= src.len() {
            dst[d..d + row_bytes].copy_from_slice(&src[s..s + row_bytes]);
        }
    }
}
