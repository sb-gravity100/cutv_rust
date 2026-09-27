// Background scrub-proxy transcode — the real fix for "accurate and fast
// like regular video editors" on long-GOP source video (see PLAN.md/
// conversation 2026-09-19). A frame-exact seek has to decode forward from
// the keyframe before it to the target; on a source with a long GOP (this
// project hit a ~250-frame/4.2s GOP in testing) that's 500ms+ per seek no
// matter how fast decode/convert are — gating/coalescing rapid requests
// only stops that cost from compounding, it doesn't remove it. Professional
// NLEs solve this the same way: transcode a short-GOP ("all the seeks are
// cheap") proxy in the background for scrubbing/preview, and only touch the
// original file for the final export. `do_cut` in app.rs already does that
// (export::cut_video always targets `self.path`, the original) — this
// module just gives `Player` a fast file to point at instead.
//
// The transcode itself runs in-process via `ffmpeg-next` (same approach as
// `export.rs`'s final cut), not a spawned `ffmpeg` subprocess.

use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use ffmpeg_next as ff;
use ff::{codec, format, frame, media, Dictionary, Packet, Rational};
use log::{debug, warn};

/// Bytes sampled from each end of the source file for `video_cache_key` —
/// bounded so hashing a multi-GB video stays fast (worst case ~2MiB read)
/// instead of hashing the whole file.
const HASH_SAMPLE_BYTES: u64 = 1024 * 1024;

/// Cheap content-based cache key for a source video: file length + a sample
/// of bytes from the start and end, hashed. Not cryptographic, but collision
/// odds are irrelevant here — worst case is a redundant re-transcode, never
/// wrong playback. Lets two different paths pointing at the same content
/// (a renamed/copied file) reuse the same scrub proxy instead of
/// re-transcoding.
fn video_cache_key(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();

    let mut hasher = DefaultHasher::new();
    len.hash(&mut hasher);

    let head_len = HASH_SAMPLE_BYTES.min(len) as usize;
    let mut head = vec![0u8; head_len];
    file.read_exact(&mut head).ok()?;
    head.hash(&mut hasher);

    if len > HASH_SAMPLE_BYTES {
        let tail_len = HASH_SAMPLE_BYTES.min(len) as usize;
        file.seek(SeekFrom::End(-(tail_len as i64))).ok()?;
        let mut tail = vec![0u8; tail_len];
        file.read_exact(&mut tail).ok()?;
        tail.hash(&mut hasher);
    }

    Some(format!("{:016x}", hasher.finish()))
}

/// Every frame in the proxy is a keyframe within this many frames of any
/// other — i.e. the worst-case seek only ever has to decode this many
/// frames forward, regardless of the source's own GOP structure. Small
/// enough to feel instant, large enough not to bloat the proxy file size
/// pointlessly (true all-intra, GOP=1, is markedly bigger for little
/// perceptible seek-speed gain once you're already down from hundreds of
/// frames to single digits).
const PROXY_GOP: u32 = 8;

/// Kicks off a background transcode of `path` into a short-GOP proxy file
/// and returns a handle that resolves to its path once ready. The caller
/// (app.rs) keeps using the original file until then, then swaps `Player`
/// over to the proxy at the current position — playback is usable
/// immediately, just without fast accurate seeking until the proxy lands.
///
/// The proxy is named after `video_cache_key(path)`, not the source
/// filename or process id: if a proxy for this exact content already exists
/// in the temp dir (built earlier this session, or left over from a
/// previous session — including a crashed one, since nothing but a clean
/// app exit deletes them, see `cleanup_all_proxies`) it's reused as-is and
/// no transcode runs at all.
pub fn spawn_proxy(path: String, ctx: egui::Context) -> Arc<Mutex<Option<PathBuf>>> {
    let result: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let result2 = result.clone();

    thread::spawn(move || {
        let key = video_cache_key(&path).unwrap_or_else(|| {
            // File couldn't be hashed (unreadable, vanished, ...) — fall
            // back to a pid-based name so the transcode below at least has
            // somewhere to write; no cross-session reuse in this case.
            format!("nohash_{}_{}", std::process::id(), std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
        });
        let out_path = std::env::temp_dir().join(format!("cutv_proxy_{key}.mp4"));

        if out_path.is_file() {
            debug!("scrub proxy: reusing cached proxy for {path} -> {out_path:?}");
            *result2.lock().unwrap() = Some(out_path);
            ctx.request_repaint();
            return;
        }

        // Transcode to a .tmp sibling and rename into place only on success,
        // so a half-written file from an interrupted/crashed transcode is
        // never mistaken for a valid cached proxy on a later run.
        let tmp_path = out_path.with_extension("mp4.tmp");
        let t0 = std::time::Instant::now();
        debug!("scrub proxy: transcoding {path} -> {out_path:?} (GOP={PROXY_GOP})");

        // Try NVENC first (matches export.rs's own NVENC-then-CPU-libx264
        // pattern), fall back to libx264 ultrafast if it's unavailable or
        // fails. Either way: short fixed GOP is what actually matters here,
        // not the codec/encoder choice.
        let ok = run_transcode(&path, &tmp_path, true).is_ok()
            || run_transcode(&path, &tmp_path, false).is_ok();

        if ok && std::fs::rename(&tmp_path, &out_path).is_ok() {
            debug!("scrub proxy: ready in {:?}", t0.elapsed());
            *result2.lock().unwrap() = Some(out_path);
            ctx.request_repaint();
        } else {
            warn!("scrub proxy: transcode failed (both nvenc and libx264) — staying on the original file, seeks will stay slow");
            let _ = std::fs::remove_file(&tmp_path);
        }
    });

    result
}

/// Deletes every scrub-proxy temp file in the system temp dir — this
/// session's and any leftover from a previous one (including a crashed
/// session, which never runs `CutvApp`'s `Drop`). Called once on normal app
/// exit. Proxies are deliberately *not* deleted as videos are switched
/// mid-session (see `app.rs::load_video`) so that switching back to an
/// already-opened video reuses its existing proxy instead of
/// re-transcoding; this is what actually clears them out.
pub fn cleanup_all_proxies() {
    let dir = std::env::temp_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            debug!("scrub proxy cleanup: failed to read {dir:?}: {e}");
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("cutv_proxy_") {
            let path = entry.path();
            if let Err(e) = std::fs::remove_file(&path) {
                debug!("scrub proxy cleanup: failed to remove {path:?}: {e}");
            } else {
                debug!("scrub proxy cleanup: removed {path:?}");
            }
        }
    }
}

struct VStream {
    decoder: ff::decoder::Video,
    encoder: ff::encoder::Video,
    ost_index: usize,
    ost_time_base: Rational,
}

struct AStream {
    decoder: ff::decoder::Audio,
    graph: ff::filter::Graph,
    encoder: ff::encoder::Audio,
    ost_index: usize,
    ost_time_base: Rational,
}

/// Short-GOP passthrough-resolution transcode of the whole file — no crop,
/// speed, or trim (unlike `export.rs`'s `cut_video`, which needs all
/// three). `try_nvenc=false` forces the CPU `libx264` path regardless of
/// NVENC availability, so `spawn_proxy` can try both in sequence.
fn run_transcode(src_path: &str, tmp_path: &std::path::Path, try_nvenc: bool) -> Result<()> {
    ff::init()?;

    let mut ictx = format::input(&src_path)?;
    let tmp_str = tmp_path.to_string_lossy().to_string();
    let mut octx = format::output_as(&tmp_str, "mp4")?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);

    let v_index = ictx.streams().best(media::Type::Video).map(|s| s.index());
    let a_index = ictx.streams().best(media::Type::Audio).map(|s| s.index());

    let mut video = match v_index {
        Some(idx) => {
            let stream = ictx.stream(idx).ok_or_else(|| anyhow!("video stream vanished"))?;
            Some(new_vstream(&stream, &mut octx, try_nvenc, global_header)?)
        }
        None => None,
    };
    let mut audio = match a_index {
        Some(idx) => {
            let stream = ictx.stream(idx).ok_or_else(|| anyhow!("audio stream vanished"))?;
            Some(new_astream(&stream, &mut octx, global_header)?)
        }
        None => None,
    };
    if video.is_none() {
        return Err(anyhow!("no video stream found in: {src_path}"));
    }

    octx.write_header()?;
    if let Some(v) = &mut video {
        v.ost_time_base = octx.stream(v.ost_index).ok_or_else(|| anyhow!("output video stream missing"))?.time_base();
    }
    if let Some(a) = &mut audio {
        a.ost_time_base = octx.stream(a.ost_index).ok_or_else(|| anyhow!("output audio stream missing"))?.time_base();
    }

    for (stream, packet) in ictx.packets() {
        let idx = stream.index();
        if Some(idx) == v_index {
            let v = video.as_mut().unwrap();
            v.decoder.send_packet(&packet)?;
            drain_video(v, &mut octx)?;
        } else if Some(idx) == a_index && let Some(a) = audio.as_mut() {
            a.decoder.send_packet(&packet)?;
            drain_audio(a, &mut octx)?;
        }
    }

    if let Some(v) = &mut video {
        v.decoder.send_eof()?;
        drain_video(v, &mut octx)?;
        v.encoder.send_eof()?;
        drain_video_packets(&mut v.encoder, v.ost_index, v.ost_time_base, &mut octx)?;
    }
    if let Some(a) = &mut audio {
        a.decoder.send_eof()?;
        drain_audio(a, &mut octx)?;
        a.graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().flush()?;
        let mut filtered = frame::Audio::empty();
        while a.graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
            a.encoder.send_frame(&filtered)?;
            drain_audio_packets(&mut a.encoder, a.ost_index, a.ost_time_base, &mut octx)?;
        }
        a.encoder.send_eof()?;
        drain_audio_packets(&mut a.encoder, a.ost_index, a.ost_time_base, &mut octx)?;
    }

    octx.write_trailer()?;
    Ok(())
}

fn new_vstream(
    stream: &format::stream::Stream, octx: &mut format::context::Output,
    try_nvenc: bool, global_header: bool,
) -> Result<VStream> {
    let decoder = codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;

    let (name, mut opts): (&str, Dictionary) = if try_nvenc {
        let mut d = Dictionary::new();
        d.set("preset", "p1");
        d.set("rc", "vbr");
        d.set("cq", "23");
        ("h264_nvenc", d)
    } else {
        let mut d = Dictionary::new();
        d.set("preset", "ultrafast");
        d.set("crf", "20");
        ("libx264", d)
    };
    // Fixed short GOP with scene-cut adaptive keyframing disabled — the
    // whole point of the proxy (see module doc comment): every seek should
    // only ever have to decode PROXY_GOP frames forward, regardless of
    // source content.
    opts.set("keyint_min", &PROXY_GOP.to_string());
    opts.set("sc_threshold", "0");

    let enc_codec = ff::encoder::find_by_name(name)
        .ok_or_else(|| anyhow!("no video encoder registered for {name}"))?;

    let mut ost = octx.add_stream(enc_codec)?;
    let mut enc_ctx = codec::context::Context::new_with_codec(enc_codec)
        .encoder()
        .video()?;
    enc_ctx.set_width(decoder.width());
    enc_ctx.set_height(decoder.height());
    enc_ctx.set_format(decoder.format());
    enc_ctx.set_aspect_ratio(decoder.aspect_ratio());
    enc_ctx.set_time_base(stream.time_base());
    enc_ctx.set_gop(PROXY_GOP);
    if global_header {
        enc_ctx.set_flags(codec::Flags::GLOBAL_HEADER);
    }

    let encoder = enc_ctx.open_as_with(enc_codec, opts)?;
    ost.set_parameters(&encoder);
    ost.set_time_base(stream.time_base());

    Ok(VStream {
        decoder, encoder,
        ost_index: ost.index(),
        ost_time_base: stream.time_base(), // corrected after write_header()
    })
}

fn new_astream(
    stream: &format::stream::Stream, octx: &mut format::context::Output, global_header: bool,
) -> Result<AStream> {
    let decoder = codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .audio()?;

    let enc_codec = ff::encoder::find_by_name("aac")
        .ok_or_else(|| anyhow!("no audio encoder registered for aac"))?;

    let mut ost = octx.add_stream(enc_codec)?;
    let mut enc_ctx = codec::context::Context::new_with_codec(enc_codec)
        .encoder()
        .audio()?;

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
            .ok_or_else(|| anyhow!("aac: no supported sample formats"))?,
    );
    enc_ctx.set_bit_rate(160_000);
    enc_ctx.set_time_base((1, decoder.rate() as i32));

    let encoder = enc_ctx.open_as(enc_codec)?;
    ost.set_parameters(&encoder);
    ost.set_time_base((1, decoder.rate() as i32));

    // Same pattern as export.rs's AudioPipe: the format/channel-layout/rate
    // conversion is an explicit trailing `aformat` filter stage (not the
    // struct-based abuffersink setters, which hit a broken channel_layouts
    // AVOption against this FFmpeg build and segfault once real filtering
    // is involved — see export.rs's build_graph doc comment), and AAC's
    // fixed frame_size needs `asetnsamples` to rechunk into exactly that
    // many samples per frame.
    let aformat = format!(
        "aformat=sample_fmts={}:sample_rates={}:channel_layouts=0x{:x}",
        encoder.format().name(), encoder.rate(), encoder.channel_layout().bits(),
    );
    let mut stages = vec!["anull".to_string(), aformat];
    let frame_size = encoder.frame_size();
    if frame_size > 0 {
        stages.push(format!("asetnsamples=n={frame_size}:p=0"));
    }
    let graph = build_audio_graph(&decoder, stream.time_base(), &stages.join(","))?;

    Ok(AStream {
        decoder, graph, encoder,
        ost_index: ost.index(),
        ost_time_base: stream.time_base(), // corrected after write_header()
    })
}

fn build_audio_graph(decoder: &ff::decoder::Audio, tb: Rational, spec: &str) -> Result<ff::filter::Graph> {
    let mut graph = ff::filter::Graph::new();
    // abuffer's declared time_base must match what decoded frames actually
    // carry as pts — the STREAM's time_base, not decoder.time_base() (see
    // export.rs's build_graph doc comment for the WebM-source bug this
    // distinction caused there).
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

fn drain_video(v: &mut VStream, octx: &mut format::context::Output) -> Result<()> {
    let mut frame = frame::Video::empty();
    while v.decoder.receive_frame(&mut frame).is_ok() {
        v.encoder.send_frame(&frame)?;
        drain_video_packets(&mut v.encoder, v.ost_index, v.ost_time_base, octx)?;
    }
    Ok(())
}

fn drain_video_packets(
    encoder: &mut ff::encoder::Video, ost_index: usize, ost_tb: Rational,
    octx: &mut format::context::Output,
) -> Result<()> {
    let mut pkt = Packet::empty();
    while encoder.receive_packet(&mut pkt).is_ok() {
        pkt.set_stream(ost_index);
        // rescale from the ENCODER's own time_base (what receive_packet
        // actually hands back), not the input stream's — see export.rs's
        // drain_video_packets doc comment for the WebM-source bug that
        // distinction caused on the audio side.
        pkt.rescale_ts(encoder.time_base(), ost_tb);
        pkt.write_interleaved(octx)?;
    }
    Ok(())
}

fn drain_audio(a: &mut AStream, octx: &mut format::context::Output) -> Result<()> {
    let mut frame = frame::Audio::empty();
    while a.decoder.receive_frame(&mut frame).is_ok() {
        a.graph.get("in").ok_or_else(|| anyhow!("filter graph missing 'in'"))?.source().add(&frame)?;
        let mut filtered = frame::Audio::empty();
        while a.graph.get("out").unwrap().sink().frame(&mut filtered).is_ok() {
            a.encoder.send_frame(&filtered)?;
            drain_audio_packets(&mut a.encoder, a.ost_index, a.ost_time_base, octx)?;
        }
    }
    Ok(())
}

fn drain_audio_packets(
    encoder: &mut ff::encoder::Audio, ost_index: usize, ost_tb: Rational,
    octx: &mut format::context::Output,
) -> Result<()> {
    let mut pkt = Packet::empty();
    while encoder.receive_packet(&mut pkt).is_ok() {
        pkt.set_stream(ost_index);
        pkt.rescale_ts(encoder.time_base(), ost_tb);
        pkt.write_interleaved(octx)?;
    }
    Ok(())
}

