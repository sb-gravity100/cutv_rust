//! In-process trim + codec-matched encode for the final cut export
//! (`app.rs`'s `do_cut`), via the `ffmpeg-next` crate (libav* bindings)
//! instead of spawning an `ffmpeg` CLI subprocess. Playback (`player.rs`)
//! stays on GStreamer; `probe.rs`'s `ffprobe`/`nvenc_available` calls plus
//! `proxy.rs`/`thumbs.rs`/`gif.rs` still shell out to the `ffmpeg`/
//! `ffprobe` binaries — only this export path changed, and `ffmpeg`/
//! `ffprobe` on `PATH` is still a required runtime dependency either way.
//! See `PLAN.md`'s "The ffmpeg-next export path" for why NVENC here doesn't
//! carry the same hw_frames_ctx-FFI risk that sank the earlier hand-rolled
//! `ffmpeg-next` *decoder* attempt: FFmpeg's NVENC *encoders* accept plain
//! software frames and manage the CUDA device internally.

use anyhow::{Result, anyhow, bail};
use ffmpeg_next as ff;
use ff::{Dictionary, Packet, Rational, codec, format, frame, media};

use crate::crop::CropRect;
use log::debug;
use crate::probe::atempo_chain;

/// ffmpeg's global microsecond time base (`AV_TIME_BASE`), used by
/// `format::context::Input::seek`'s stream_index=-1 convention.
const AV_TIME_BASE: i64 = 1_000_000;
/// `FF_QP2LAMBDA` from ffmpeg's `avcodec.h` — the qscale→global_quality
/// scale factor CLI `-qscale:v` applies internally; needed for the
/// mpeg4/mpeg2video path, which has no plain string option for it.
const FF_QP2LAMBDA: i32 = 118;
/// How far past `out_t` to keep decoding before a stream is done — covers
/// encoder reordering/lookahead so the last in-range frame isn't dropped
/// right at the boundary.
const TAIL_SLACK_SECS: f64 = 1.0;
/// Frames/samples with a timestamp before `in_t` by less than this are
/// still treated as "in range" — avoids float-rounding false negatives
/// right at the trim point.
const HEAD_EPS_SECS: f64 = 1e-4;

fn to_pts(secs: f64, tb: Rational) -> i64 {
    (secs * f64::from(tb.denominator()) / f64::from(tb.numerator())).round() as i64
}

fn ts_secs(pts: i64, tb: Rational) -> f64 {
    pts as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator())
}

/// `-b:v`/`-maxrate`/`-bufsize` targeting the given bitrate (maxrate at
/// 1.5x, bufsize at 2x — room for encoder-side VBR fluctuation without
/// ballooning file size). `None` sets `b=0`, i.e. unconstrained
/// (quality-only rate control), used when the source bitrate is unknown.
fn set_bitrate_opts(d: &mut Dictionary, br_kbps: Option<u64>) {
    match br_kbps {
        Some(br) => {
            d.set("b", &format!("{}", br * 1000));
            d.set("maxrate", &format!("{}", br * 1000 * 3 / 2));
            d.set("bufsize", &format!("{}", br * 1000 * 2));
        }
        None => d.set("b", "0"),
    }
}

fn nvenc_opts(br_kbps: Option<u64>, profile: &str) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    d.set("preset", "p6");
    d.set("tune", "hq");
    d.set("rc", "vbr");
    d.set("cq", "18");
    set_bitrate_opts(&mut d, br_kbps);
    d.set("spatial_aq", "1");
    d.set("temporal_aq", "1");
    d.set("aq-strength", "8");
    d.set("rc-lookahead", "32");
    d.set("profile", profile);
    d
}

/// CPU h264/hevc: bitrate-targeted (ABR + VBV) when the source bitrate is
/// known, CRF-only quality mode otherwise.
fn cpu_h26x_opts(br_kbps: Option<u64>) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    d.set("preset", "slow");
    match br_kbps {
        Some(br) => set_bitrate_opts(&mut d, Some(br)),
        None => d.set("crf", "18"),
    }
    d
}

fn crf_opts(crf: &str, br_kbps: Option<u64>) -> Dictionary<'static> {
    let mut d = Dictionary::new();
    d.set("crf", crf);
    set_bitrate_opts(&mut d, br_kbps);
    d
}

/// Encoder name + options for a source video codec, mirroring the values
/// previously in the CLI-era `probe::encode_args`. `qscale` is `Some(n)`
/// for the mpeg4/mpeg2video path, which needs `global_quality`/`QSCALE`
/// set directly on the codec context rather than through a string option.
struct VideoEncSpec {
    name: &'static str,
    opts: Dictionary<'static>,
    qscale: Option<i32>,
}

/// User overrides for the cut's output video. Each `None` means "same as
/// source" (the default) — no extra filter / the source-derived bitrate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExportOpts {
    pub fps: Option<f64>,
    pub bitrate_kbps: Option<u64>,
    /// Target output height; width follows the (post-crop) aspect ratio,
    /// rounded to even for encoder compatibility.
    pub height: Option<u32>,
}

fn video_enc_spec(id: codec::Id, use_nvenc: bool, br_kbps: Option<u64>) -> VideoEncSpec {
    use codec::Id as I;
    let (name, opts, qscale): (&'static str, Dictionary<'static>, Option<i32>) = match id {
        I::H264 if use_nvenc => ("h264_nvenc", nvenc_opts(br_kbps, "high"), None),
        I::HEVC if use_nvenc => ("hevc_nvenc", nvenc_opts(br_kbps, "main"), None),
        I::H264 => ("libx264", cpu_h26x_opts(br_kbps), None),
        I::HEVC => ("libx265", cpu_h26x_opts(br_kbps), None),
        I::VP9 => ("libvpx-vp9", crf_opts("33", br_kbps), None),
        I::VP8 => ("libvpx", crf_opts("10", br_kbps), None),
        I::AV1 => ("libaom-av1", crf_opts("30", br_kbps), None),
        I::MPEG4 => ("mpeg4", Dictionary::new(), Some(3)),
        I::MPEG2VIDEO => ("mpeg2video", Dictionary::new(), Some(3)),
        I::PRORES => ("prores_ks", { let mut d = Dictionary::new(); d.set("profile", "3"); d }, None),
        _ if use_nvenc => ("h264_nvenc", nvenc_opts(br_kbps, "high"), None),
        _ => ("libx264", cpu_h26x_opts(br_kbps), None),
    };
    VideoEncSpec { name, opts, qscale }
}

fn pix_fmt_name(p: format::Pixel) -> &'static str {
    p.descriptor().map(|d| d.name()).unwrap_or("yuv420p")
}

/// Normalized output audio codec *candidates* for the given output
/// container path, most-preferred first — mirrors the CLI-era
/// `probe::normalized_audio_codec`: unlike video (which stays in the
/// source's own codec family for a visually-lossless passthrough cut),
/// audio is always re-encoded to one consistent codec regardless of the
/// source's, so every export has predictable, broadly-compatible audio.
/// The one exception is `.webm`, whose container spec only accepts Vorbis
/// or Opus (AAC won't mux into it at all).
///
/// A list rather than a single name because the FFmpeg *libraries* linked
/// in-process here (via `ffmpeg-sys-next`/vcpkg) are a different build from
/// the `ffmpeg`/`ffprobe` *binaries* on `PATH` that `proxy.rs`/`thumbs.rs`/
/// `gif.rs` shell out to — this build has no `libopus`/`libvorbis`
/// (external GPL/non-default encoder libs vcpkg didn't enable), only
/// FFmpeg's own native `opus`/`vorbis` encoders, which are marked
/// experimental (`AudioPipe::new` relaxes `strict_std_compliance` for
/// them specifically).
fn normalized_audio_codec_candidates(out_path: &str) -> &'static [&'static str] {
    let ext = std::path::Path::new(out_path).extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("webm") { &["libopus", "opus"] } else { &["aac"] }
}

struct VideoPipe {
    decoder: ff::decoder::Video,
    graph: Option<ff::filter::Graph>,
    encoder: ff::encoder::Video,
    ost_index: usize,
    ist_time_base: Rational,
    ost_time_base: Rational,
    in_t_pts: i64,
}

impl VideoPipe {
    #[allow(clippy::too_many_arguments)]
    fn new(
        stream: &format::stream::Stream,
        octx: &mut format::context::Output,
        use_nvenc: bool,
        crop: Option<CropRect>,
        speed: f64,
        opts: ExportOpts,
        global_header: bool,
        in_t: f64,
    ) -> Result<Self> {
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .video()?;

        let br_kbps = (stream.parameters().bit_rate() > 0)
            .then(|| (stream.parameters().bit_rate() as u64 / 1000).max(100));
        let br_kbps = match opts.bitrate_kbps {
            Some(b) => { debug!("export: bitrate override {b} kbps (source {br_kbps:?})"); Some(b.max(100)) }
            None => br_kbps,
        };
        let spec = video_enc_spec(stream.parameters().id(), use_nvenc, br_kbps);

        let enc_codec = ff::encoder::find_by_name(spec.name)
            .ok_or_else(|| anyhow!("no video encoder registered for {}", spec.name))?;

        let mut ost = octx.add_stream(enc_codec)?;
        let mut enc_ctx = codec::context::Context::new_with_codec(enc_codec)
            .encoder()
            .video()?;

        let (out_w, out_h) = match crop {
            Some(c) => (c.w.round() as u32, c.h.round() as u32),
            None => (decoder.width(), decoder.height()),
        };
        // Resolution override: scale the (post-crop) frame to the target
        // height, keeping aspect; both dims forced even.
        let scaled = opts.height.map(|h| {
            let h = (h.max(2) / 2) * 2;
            let w = ((out_w as f64 * h as f64 / out_h.max(1) as f64 / 2.0).round() as u32).max(1) * 2;
            (w, h)
        }).filter(|&d| d != (out_w, out_h));
        let (out_w, out_h) = scaled.unwrap_or((out_w, out_h));
        if let Some((w, h)) = scaled { debug!("export: scaling to {w}x{h}"); }
        enc_ctx.set_width(out_w);
        enc_ctx.set_height(out_h);
        enc_ctx.set_format(decoder.format());
        enc_ctx.set_aspect_ratio(decoder.aspect_ratio());
        enc_ctx.set_time_base(stream.time_base());

        let mut flags = codec::Flags::empty();
        if global_header { flags |= codec::Flags::GLOBAL_HEADER; }
        if spec.qscale.is_some() { flags |= codec::Flags::QSCALE; }
        enc_ctx.set_flags(flags);
        if let Some(q) = spec.qscale {
            enc_ctx.set_global_quality(q * FF_QP2LAMBDA);
        }

        let encoder = enc_ctx.open_as_with(enc_codec, spec.opts)?;
        ost.set_parameters(&encoder);
        // set_parameters copies AVCodecParameters, which doesn't include
        // time_base (a separate AVStream field) — without this, muxing
        // warns "stream N, timescale not set" and falls back to guessing.
        ost.set_time_base(stream.time_base());

        // Crop and speed both flow through the same filter-spec string a
        // CLI `-vf` value would have used (`CropRect::to_vf()`'s
        // `crop=w:h:x:y`, then `setpts=...*PTS`) — no filter graph at all
        // when neither applies, same as the old CLI path skipping `-vf`.
        let mut vf = Vec::new();
        if let Some(c) = crop { vf.push(c.to_vf()); }
        let speed_changed = (speed - 1.0).abs() > 1e-6;
        if speed_changed { vf.push(format!("setpts={:.6}*PTS", 1.0 / speed)); }
        if let Some((w, h)) = scaled { vf.push(format!("scale={w}:{h}:flags=lanczos")); }
        if let Some(fps) = opts.fps.filter(|f| *f > 0.0) {
            debug!("export: fps override {fps}");
            // fps changes the link time_base to 1/fps; settb restores the
            // stream time_base the encoder/muxer were configured with.
            let tb = stream.time_base();
            vf.push(format!("fps={fps:.6},settb={}/{}", tb.numerator(), tb.denominator()));
        }
        let graph = if vf.is_empty() {
            None
        } else {
            Some(Self::build_graph(&decoder, stream.time_base(), &vf.join(","))?)
        };

        Ok(Self {
            decoder,
            graph,
            encoder,
            ost_index: ost.index(),
            ist_time_base: stream.time_base(),
            ost_time_base: stream.time_base(), // corrected after write_header()
            in_t_pts: to_pts(in_t, stream.time_base()),
        })
    }

    fn build_graph(decoder: &ff::decoder::Video, tb: Rational, spec: &str) -> Result<ff::filter::Graph> {
        let mut graph = ff::filter::Graph::new();
        let sar = decoder.aspect_ratio();
        let sar = if sar.numerator() == 0 { Rational::new(1, 1) } else { sar };
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}/{}:pixel_aspect={}/{}",
            decoder.width(), decoder.height(), pix_fmt_name(decoder.format()),
            tb.numerator(), tb.denominator(),
            sar.numerator(), sar.denominator(),
        );
        graph.add(&ff::filter::find("buffer").ok_or_else(|| anyhow!("no buffer filter"))?, "in", &args)?;
        graph.add(&ff::filter::find("buffersink").ok_or_else(|| anyhow!("no buffersink filter"))?, "out", "")?;
        graph.output("in", 0)?.input("out", 0)?.parse(spec)?;
        graph.validate()?;
        Ok(graph)
    }

    fn push_frame(&mut self, f: &frame::Video, octx: &mut format::context::Output, on_progress: &mut dyn FnMut(f32), cut_dur: f64) -> Result<()> {
        match &mut self.graph {
            Some(graph) => {
                graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().add(f)?;
                let mut filtered = frame::Video::empty();
                while graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
                    Self::encode(&mut self.encoder, &filtered, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur)?;
                }
                Ok(())
            }
            None => Self::encode(&mut self.encoder, f, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur),
        }
    }

    fn encode(
        encoder: &mut ff::encoder::Video, f: &frame::Video, ost_index: usize,
        ost_tb: Rational, octx: &mut format::context::Output,
        on_progress: &mut dyn FnMut(f32), cut_dur: f64,
    ) -> Result<()> {
        encoder.send_frame(f)?;
        drain_video_packets(encoder, ost_index, ost_tb, octx, on_progress, cut_dur)
    }

    /// Feeds every currently-decodable frame through crop/speed filtering
    /// (if any) and the encoder, applying the same in/out trim +
    /// timestamp-rebase logic whether called after a fresh `send_packet` or
    /// after `send_eof` (`receive_frame` drains identically either way).
    /// Returns whether this stream has now read past `out_t` and needs no
    /// more packets.
    fn drain_decoder(&mut self, octx: &mut format::context::Output, in_t: f64, out_t: f64, on_progress: &mut dyn FnMut(f32), cut_dur: f64) -> Result<bool> {
        let mut frame = frame::Video::empty();
        let mut done = false;
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let Some(pts) = frame.pts() else { continue };
            let t = ts_secs(pts, self.ist_time_base);
            // Keep decoding past out_t for a bit (encoder/decoder
            // reordering means a still-in-range frame can arrive after an
            // out-of-range one), but only *encode* frames inside
            // [in_t, out_t] — the slack window itself is discarded, not
            // pushed downstream.
            if t > out_t + TAIL_SLACK_SECS { done = true; continue; }
            if t > out_t { continue; }
            if t < in_t - HEAD_EPS_SECS { continue; }
            frame.set_pts(Some(pts - self.in_t_pts));
            self.push_frame(&frame, octx, on_progress, cut_dur)?;
        }
        Ok(done)
    }

    fn flush(&mut self, octx: &mut format::context::Output, in_t: f64, out_t: f64, on_progress: &mut dyn FnMut(f32), cut_dur: f64) -> Result<()> {
        self.decoder.send_eof()?;
        self.drain_decoder(octx, in_t, out_t, on_progress, cut_dur)?;
        if let Some(graph) = &mut self.graph {
            graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().flush()?;
            let mut filtered = frame::Video::empty();
            while graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
                Self::encode(&mut self.encoder, &filtered, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur)?;
            }
        }
        self.encoder.send_eof()?;
        drain_video_packets(&mut self.encoder, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur)
    }
}

/// `rescale_ts`'s source time_base must be the *encoder's own* — what
/// `receive_packet()` actually hands back packet timestamps in — not the
/// original input stream's. `VideoPipe::new` happens to set the encoder's
/// time_base equal to the input stream's (`enc_ctx.set_time_base(stream.
/// time_base())`), so these coincide for video; `drain_audio_packets`
/// below is the same fix for the case where they don't (see its comment).
fn drain_video_packets(
    encoder: &mut ff::encoder::Video, ost_index: usize, ost_tb: Rational,
    octx: &mut format::context::Output, on_progress: &mut dyn FnMut(f32), cut_dur: f64,
) -> Result<()> {
    let mut pkt = Packet::empty();
    while encoder.receive_packet(&mut pkt).is_ok() {
        pkt.set_stream(ost_index);
        pkt.rescale_ts(encoder.time_base(), ost_tb);
        let t = ts_secs(pkt.pts().unwrap_or(0), ost_tb);
        on_progress((t / cut_dur).clamp(0.0, 1.0) as f32);
        pkt.write_interleaved(octx)?;
    }
    Ok(())
}

struct AudioPipe {
    decoder: ff::decoder::Audio,
    graph: ff::filter::Graph,
    encoder: ff::encoder::Audio,
    ost_index: usize,
    ist_time_base: Rational,
    ost_time_base: Rational,
    in_t_pts: i64,
}

impl AudioPipe {
    #[allow(clippy::too_many_arguments)]
    fn new(
        stream: &format::stream::Stream, octx: &mut format::context::Output,
        global_header: bool, in_t: f64, out_path: &str, speed: f64, keep_pitch: bool,
    ) -> Result<Self> {
        let decoder = codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()?;

        let (enc_name, enc_codec) = normalized_audio_codec_candidates(out_path)
            .iter()
            .find_map(|&name| ff::encoder::find_by_name(name).map(|c| (name, c)))
            .ok_or_else(|| anyhow!(
                "no registered audio encoder among candidates for output: {out_path}"
            ))?;

        let mut ost = octx.add_stream(enc_codec)?;
        let mut enc_ctx = codec::context::Context::new_with_codec(enc_codec)
            .encoder()
            .audio()?;
        // FFmpeg's own native `opus`/`vorbis` encoders (the fallback when
        // `libopus`/`libvorbis` aren't registered — see
        // `normalized_audio_codec_candidates`) are marked experimental;
        // `open_as` below refuses them at the default `Strict` compliance
        // level otherwise.
        if enc_name == "opus" || enc_name == "vorbis" {
            enc_ctx.compliance(codec::Compliance::Experimental);
        }

        let enc_audio = enc_codec.audio()?;
        let channel_layout = enc_audio
            .channel_layouts()
            .map(|cls| cls.best(decoder.channel_layout().channels()))
            .unwrap_or(ff::channel_layout::ChannelLayout::STEREO);

        if global_header { enc_ctx.set_flags(codec::Flags::GLOBAL_HEADER); }
        enc_ctx.set_rate(decoder.rate() as i32);
        enc_ctx.set_channel_layout(channel_layout);
        enc_ctx.set_format(
            enc_audio.formats().and_then(|mut f| f.next())
                .ok_or_else(|| anyhow!("{enc_name}: no supported sample formats"))?,
        );
        // Normalized audio is always re-encoded (never a lossless
        // passthrough), so it always gets a target bitrate — matching the
        // source's when known, falling back to a broadly-reasonable
        // default otherwise, same as the CLI-era `encode_args`.
        let br = (stream.parameters().bit_rate() > 0)
            .then_some(stream.parameters().bit_rate() as usize / 1000)
            .map(|kbps| kbps.max(64))
            .unwrap_or(192);
        enc_ctx.set_bit_rate(br * 1000);
        enc_ctx.set_time_base((1, decoder.rate() as i32));

        let encoder = enc_ctx.open_as(enc_codec)?;
        ost.set_parameters(&encoder);
        ost.set_time_base((1, decoder.rate() as i32));

        // Keep pitch: atempo's WSOLA time-stretch changes tempo only (and
        // preserves the sample rate). Let pitch shift: asetrate scales the
        // sample rate by speed (tape/vinyl-style), then aresample restores
        // the rate the encoder above was configured for. Always routed
        // through a filter graph (even "anull" when speed is unchanged) so
        // abuffersink auto-negotiates any decoder->encoder format/
        // channel-layout mismatch, same pattern ffmpeg-next's own
        // transcode-audio.rs example uses.
        let speed_changed = (speed - 1.0).abs() > 1e-6;
        let core_af = if speed_changed {
            if keep_pitch {
                atempo_chain(speed)
            } else {
                let sr = decoder.rate();
                let new_sr = ((sr as f64 * speed).round().max(1.0)) as u32;
                format!("asetrate={new_sr},aresample={sr}")
            }
        } else {
            "anull".to_string()
        };
        // The encoder's required output format/channel-layout/rate is
        // applied as an explicit `aformat` filter stage (see `build_graph`
        // for why, vs. the struct-based abuffersink setters). Fixed-
        // frame-size codecs (AAC's 1024, most others too — 0 means
        // variable/any size accepted) additionally need `asetnsamples` to
        // rechunk the filtered stream into exactly that many samples per
        // frame: `encoder.send_frame` errors ("frame_size (1024) was not
        // respected" / "nb_samples (N) > frame_size") otherwise, since
        // nothing else in this hand-rolled pipeline buffers/re-slices
        // samples across frame boundaries the way ffmpeg's own CLI
        // encoding loop does internally.
        let aformat = format!(
            "aformat=sample_fmts={}:sample_rates={}:channel_layouts=0x{:x}",
            encoder.format().name(), encoder.rate(), encoder.channel_layout().bits(),
        );
        let mut stages = vec![core_af, aformat];
        let frame_size = encoder.frame_size();
        if frame_size > 0 {
            stages.push(format!("asetnsamples=n={frame_size}:p=0"));
        }
        let graph = Self::build_graph(&decoder, stream.time_base(), &stages.join(","))?;

        Ok(Self {
            decoder,
            graph,
            encoder,
            ost_index: ost.index(),
            ist_time_base: stream.time_base(),
            ost_time_base: stream.time_base(), // corrected after write_header()
            in_t_pts: to_pts(in_t, stream.time_base()),
        })
    }

    /// The output format/channel-layout/rate the encoder needs is applied
    /// as an explicit trailing `aformat=...` filter stage in `spec` (text,
    /// parsed the same way as `atempo`/`asetrate`/`anull`) rather than via
    /// `AVFilterContext`'s struct-based `av_buffersink_set_*` setters
    /// (`out.set_sample_format`/`set_channel_layout`/`set_sample_rate`):
    /// against this FFmpeg 9.0.1 build, `ffmpeg-next` 9.0.0's
    /// `set_channel_layout` hits a broken/mismatched `channel_layouts`
    /// AVOption ("Tried to set option 'channel_layouts' of type (null)
    /// from value of type <channel_layout>, this is not supported") that
    /// segfaults once real filtering (e.g. `atempo`) is in the chain —
    /// almost certainly a version mismatch between the crate's assumed
    /// libavfilter option shape and FFmpeg 9's newer `AVChannelLayout`-
    /// based one. The text `aformat` filter goes through the same
    /// string-graph parser as everything else here and sidesteps it
    /// entirely.
    fn build_graph(decoder: &ff::decoder::Audio, tb: Rational, spec: &str) -> Result<ff::filter::Graph> {
        let mut graph = ff::filter::Graph::new();
        // The `abuffer` source's declared time_base must match what the
        // frames pushed into it actually carry as `pts` — that's the
        // STREAM's time_base (what `drain_decoder`'s `frame.set_pts`
        // rebasing uses via `in_t_pts`), not `decoder.time_base()` (the
        // codec-internal one, conventionally 1/sample_rate). These
        // coincide for MP4 sources (masking this for a while) but not for
        // WebM (a different container-level time_base, e.g. 1/1000) —
        // mismatched here, libopus logged nonstop "Queue input is
        // backward in time" because the mis-scaled pts looked
        // non-monotonic to it.
        let args = format!(
            "time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
            tb.numerator(), tb.denominator(),
            decoder.rate(), decoder.format().name(), decoder.channel_layout().bits(),
        );
        graph.add(&ff::filter::find("abuffer").ok_or_else(|| anyhow!("no abuffer filter"))?, "in", &args)?;
        graph.add(&ff::filter::find("abuffersink").ok_or_else(|| anyhow!("no abuffersink filter"))?, "out", "")?;
        graph.output("in", 0)?.input("out", 0)?.parse(spec)?;
        graph.validate()?;
        Ok(graph)
    }

    fn push_frame(&mut self, f: &frame::Audio, octx: &mut format::context::Output, on_progress: &mut dyn FnMut(f32), cut_dur: f64) -> Result<()> {
        self.graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().add(f)?;
        let mut filtered = frame::Audio::empty();
        while self.graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
            self.encoder.send_frame(&filtered)?;
            drain_audio_packets(&mut self.encoder, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur)?;
        }
        Ok(())
    }

    fn drain_decoder(&mut self, octx: &mut format::context::Output, in_t: f64, out_t: f64, on_progress: &mut dyn FnMut(f32), cut_dur: f64) -> Result<bool> {
        let mut frame = frame::Audio::empty();
        let mut done = false;
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let Some(pts) = frame.pts() else { continue };
            let t = ts_secs(pts, self.ist_time_base);
            if t > out_t + TAIL_SLACK_SECS { done = true; continue; }
            if t > out_t { continue; }
            if t < in_t - HEAD_EPS_SECS { continue; }
            frame.set_pts(Some(pts - self.in_t_pts));
            self.push_frame(&frame, octx, on_progress, cut_dur)?;
        }
        Ok(done)
    }

    fn flush(&mut self, octx: &mut format::context::Output, in_t: f64, out_t: f64, on_progress: &mut dyn FnMut(f32), cut_dur: f64) -> Result<()> {
        self.decoder.send_eof()?;
        self.drain_decoder(octx, in_t, out_t, on_progress, cut_dur)?;
        self.graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().flush()?;
        let mut filtered = frame::Audio::empty();
        while self.graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
            self.encoder.send_frame(&filtered)?;
            drain_audio_packets(&mut self.encoder, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur)?;
        }
        self.encoder.send_eof()?;
        drain_audio_packets(&mut self.encoder, self.ost_index, self.ost_time_base, octx, on_progress, cut_dur)
    }
}

/// `rescale_ts`'s source time_base must be the *encoder's own* configured
/// time_base — what `receive_packet()` actually hands back packet
/// timestamps in — not the input stream's. `AudioPipe::new` sets the
/// encoder's time_base to `(1, sample_rate)`, which is *not* generally the
/// same as the input stream's own time_base (MP4 conventionally uses
/// `1/sample_rate` too, coincidentally masking this; WebM/Matroska
/// commonly uses `1/1000` instead) — using the wrong one here silently
/// mis-scaled every audio packet's pts by the ratio between the two
/// (reproduced: a 2s cut from a WebM/Opus source came out with a container
/// duration of ~96s, `sample_rate/1000`'s ratio almost exactly).
fn drain_audio_packets(
    encoder: &mut ff::encoder::Audio, ost_index: usize, ost_tb: Rational,
    octx: &mut format::context::Output, on_progress: &mut dyn FnMut(f32), cut_dur: f64,
) -> Result<()> {
    let mut pkt = Packet::empty();
    while encoder.receive_packet(&mut pkt).is_ok() {
        pkt.set_stream(ost_index);
        pkt.rescale_ts(encoder.time_base(), ost_tb);
        let t = ts_secs(pkt.pts().unwrap_or(0), ost_tb);
        on_progress((t / cut_dur).clamp(0.0, 1.0) as f32);
        pkt.write_interleaved(octx)?;
    }
    Ok(())
}

/// Trims `src_path`'s `[in_t, out_t)` range (seconds) into `out_path`,
/// codec-matched to the source (NVENC when available for h264/hevc, else
/// the matching CPU encoder — see `video_enc_spec`), with `crop` applied
/// to video if set and a `speed`/`keep_pitch` change applied to both
/// streams if `speed != 1.0` (mirrors `do_cut`'s old CLI `setpts`/
/// `atempo`/`asetrate` filters). Audio is always re-encoded to a
/// normalized codec (`normalized_audio_codec`), never passed through.
/// Calls `on_progress` with a 0.0-1.0 fraction as the output is written.
#[allow(clippy::too_many_arguments)]
pub fn cut_video(
    src_path: &str,
    out_path: &str,
    in_t: f64,
    out_t: f64,
    crop: Option<CropRect>,
    speed: f64,
    keep_pitch: bool,
    opts: ExportOpts,
    mut on_progress: impl FnMut(f32),
) -> Result<()> {
    ff::init()?;

    let mut ictx = format::input(&src_path)?;
    let mut octx = format::output(&out_path)?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let use_nvenc = crate::probe::nvenc_available();

    let v_index = ictx.streams().best(media::Type::Video).map(|s| s.index());
    let a_index = ictx.streams().best(media::Type::Audio).map(|s| s.index());

    let mut video = match v_index {
        Some(idx) => {
            let stream = ictx.stream(idx).ok_or_else(|| anyhow!("video stream {idx} vanished"))?;
            Some(VideoPipe::new(&stream, &mut octx, use_nvenc, crop, speed, opts, global_header, in_t)?)
        }
        None => None,
    };
    let mut audio = match a_index {
        Some(idx) => {
            let stream = ictx.stream(idx).ok_or_else(|| anyhow!("audio stream {idx} vanished"))?;
            Some(AudioPipe::new(&stream, &mut octx, global_header, in_t, out_path, speed, keep_pitch)?)
        }
        None => None,
    };

    if video.is_none() && audio.is_none() {
        bail!("no video or audio stream found in: {src_path}");
    }

    octx.set_metadata(ictx.metadata().to_owned());
    octx.write_header()?;
    if let Some(v) = &mut video {
        v.ost_time_base = octx.stream(v.ost_index).ok_or_else(|| anyhow!("output video stream missing"))?.time_base();
    }
    if let Some(a) = &mut audio {
        a.ost_time_base = octx.stream(a.ost_index).ok_or_else(|| anyhow!("output audio stream missing"))?.time_base();
    }

    // Seek to the nearest keyframe at/before in_t; per-frame trimming
    // below (drain_decoder's `t < in_t` check) discards the remainder for
    // an exact ("accurate seek") start, same as ffmpeg CLI's default for
    // `-ss` given as an input option.
    let seek_ts = (in_t.max(0.0) * AV_TIME_BASE as f64).round() as i64;
    ictx.seek(seek_ts, ..seek_ts)?;

    // setpts (applied to both streams below when speed != 1) stretches or
    // compresses the *output* timeline relative to the source segment —
    // divide so the progress fraction still reaches 1.0 once encoding
    // actually finishes, rather than stalling short (speed > 1) or
    // finishing early (speed < 1).
    let cut_dur = ((out_t - in_t).max(0.001) / speed.max(0.01)).max(0.001);
    let mut v_done = video.is_none();
    let mut a_done = audio.is_none();

    for (stream, packet) in ictx.packets() {
        let idx = stream.index();
        if Some(idx) == v_index && !v_done {
            let v = video.as_mut().unwrap();
            v.decoder.send_packet(&packet)?;
            v_done = v.drain_decoder(&mut octx, in_t, out_t, &mut on_progress, cut_dur)?;
        } else if Some(idx) == a_index && !a_done {
            let a = audio.as_mut().unwrap();
            a.decoder.send_packet(&packet)?;
            a_done = a.drain_decoder(&mut octx, in_t, out_t, &mut on_progress, cut_dur)?;
        }
        if v_done && a_done { break; }
    }

    if let Some(v) = &mut video {
        v.flush(&mut octx, in_t, out_t, &mut on_progress, cut_dur)?;
    }
    if let Some(a) = &mut audio {
        a.flush(&mut octx, in_t, out_t, &mut on_progress, cut_dur)?;
    }

    octx.write_trailer()?;
    on_progress(1.0);
    Ok(())
}
