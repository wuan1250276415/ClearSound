//! Shared output keeps one stream open. Exclusive WASAPI output reopens the
//! device when the sample rate or bit depth changes. DSD stays PCM unless DoP
//! is enabled, in which case exclusive mode can emit 176.4 or 352.8 kHz.

mod decode;
mod devices;
mod dop;
mod negotiate;
mod resample;
mod session;
#[cfg(windows)]
mod wasapi_out;

pub use devices::list_output_devices;
use session::{Opened, OutputPrefs, OutputSession};

use crate::models::{PlaybackState, RepeatMode, Settings, Track};
use decode::{open_source, PcmSource};
use parking_lot::Mutex;
use resample::StereoResampler;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const SKIP_BUDGET: u8 = 8;

#[derive(Clone)]
pub struct AudioHandle {
    tx: Sender<Envelope>,
    status: Arc<Status>,
}

struct Envelope {
    cmd: Cmd,
    ack: Sender<()>,
}

enum Cmd {
    SetQueue { tracks: Vec<Track>, index: usize },
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Seek(u64),
    Volume(f32),
    Shuffle(bool),
    Repeat(RepeatMode),
    Enqueue(Vec<Track>),
    PlayNext(Track),
    Jump(usize),
    Remove(usize),
    Clear,
    UpdateArt { album: String, artist: String, art_path: String },
    Output(OutputPrefs),
}

struct Status {
    playing: AtomicBool,
    frames: AtomicU64,
    origin_ms: AtomicU64,
    track_start_frame: AtomicU64,
    duration_ms: AtomicU64,
    out_rate: AtomicU32,
    volume: AtomicU32,
    exclusive: AtomicBool,
    dop: AtomicBool,
    lock_volume: AtomicBool,
    snap: Mutex<Snap>,
    error: Mutex<Option<String>>,
    warning: Mutex<Option<String>>,
    format_notice: Mutex<Option<String>>,
    output_label: Mutex<String>,
    device_fault: Mutex<Option<String>>,
    buf: Mutex<VecDeque<f32>>,
}

struct Snap {
    queue: Vec<Track>,
    index: Option<usize>,
    repeat: RepeatMode,
    shuffle: bool,
    order: Vec<usize>,
}

struct PendingSwitch {
    at_frame: u64,
    index: usize,
    duration_ms: u64,
}

struct Player {
    status: Arc<Status>,
    ffmpeg: Option<PathBuf>,
    decoder: Option<Box<dyn PcmSource>>,
    input_rate: u32,
    resampler: StereoResampler,
    preloaded: Option<(i64, Box<dyn PcmSource>)>,
    preload_for: Option<i64>,
    pending: Option<PendingSwitch>,
    skips_left: u8,
    output: OutputSession,
    prefs: OutputPrefs,
}

impl AudioHandle {
    pub fn start(ffmpeg: Option<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Status::new());
        let thread_status = Arc::clone(&status);
        let _ = thread::Builder::new()
            .name("chengyin-audio".into())
            .spawn(move || audio_thread(rx, thread_status, ffmpeg));
        Self { tx, status }
    }

    pub fn snapshot(&self) -> PlaybackState {
        self.status.snapshot()
    }

    pub fn apply_settings(&self, settings: &Settings) -> Result<(), String> {
        self.volume(settings.volume)?;
        self.shuffle(settings.shuffle)?;
        self.repeat(settings.repeat_mode)?;
        self.send(Cmd::Output(OutputPrefs {
            device_id: settings.output_device_id.clone(),
            exclusive: settings.output_mode == crate::models::OutputMode::Exclusive,
            dop: settings.dop,
            lock_volume: settings.exclusive_volume_lock,
        }))
    }

    pub fn set_queue(&self, tracks: Vec<Track>, index: usize) -> Result<(), String> {
        self.send(Cmd::SetQueue { tracks, index })
    }

    pub fn toggle(&self) -> Result<(), String> {
        self.send(Cmd::Toggle)
    }

    pub fn play(&self) -> Result<(), String> {
        self.send(Cmd::Play)
    }

    pub fn pause(&self) -> Result<(), String> {
        self.send(Cmd::Pause)
    }

    pub fn next(&self) -> Result<(), String> {
        self.send(Cmd::Next)
    }

    pub fn prev(&self) -> Result<(), String> {
        self.send(Cmd::Prev)
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), String> {
        self.send(Cmd::Seek(position_ms))
    }

    pub fn volume(&self, volume: f32) -> Result<(), String> {
        self.send(Cmd::Volume(volume))
    }

    pub fn shuffle(&self, enabled: bool) -> Result<(), String> {
        self.send(Cmd::Shuffle(enabled))
    }

    pub fn repeat(&self, mode: RepeatMode) -> Result<(), String> {
        self.send(Cmd::Repeat(mode))
    }

    pub fn enqueue(&self, tracks: Vec<Track>) -> Result<(), String> {
        self.send(Cmd::Enqueue(tracks))
    }

    pub fn play_next(&self, track: Track) -> Result<(), String> {
        self.send(Cmd::PlayNext(track))
    }

    pub fn jump(&self, index: usize) -> Result<(), String> {
        self.send(Cmd::Jump(index))
    }

    pub fn remove(&self, index: usize) -> Result<(), String> {
        self.send(Cmd::Remove(index))
    }

    pub fn clear(&self) -> Result<(), String> {
        self.send(Cmd::Clear)
    }

    pub fn update_art(&self, album: &str, artist: &str, art_path: &str) -> Result<(), String> {
        self.send(Cmd::UpdateArt {
            album: album.to_string(),
            artist: artist.to_string(),
            art_path: art_path.to_string(),
        })
    }

    fn send(&self, cmd: Cmd) -> Result<(), String> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.tx
            .send(Envelope { cmd, ack: ack_tx })
            .map_err(|_| "播放引擎已停止".to_string())?;
        ack_rx
            .recv_timeout(Duration::from_secs(4))
            .map_err(|_| "播放引擎没有响应".to_string())
    }
}

impl Status {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            origin_ms: AtomicU64::new(0),
            track_start_frame: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            out_rate: AtomicU32::new(48_000),
            volume: AtomicU32::new(0.85f32.to_bits()),
            exclusive: AtomicBool::new(false),
            dop: AtomicBool::new(false),
            lock_volume: AtomicBool::new(false),
            snap: Mutex::new(Snap {
                queue: Vec::new(),
                index: None,
                repeat: RepeatMode::Off,
                shuffle: false,
                order: Vec::new(),
            }),
            error: Mutex::new(None),
            warning: Mutex::new(None),
            format_notice: Mutex::new(None),
            output_label: Mutex::new(String::new()),
            device_fault: Mutex::new(None),
            buf: Mutex::new(VecDeque::new()),
        }
    }

    fn take_fault(&self) -> Option<String> {
        self.device_fault.lock().take()
    }

    fn snapshot(&self) -> PlaybackState {
        let snap = self.snap.lock();
        let current = snap
            .index
            .and_then(|index| snap.queue.get(index).cloned());
        PlaybackState {
            playing: self.playing.load(Ordering::Relaxed),
            position_ms: position_ms(self),
            duration_ms: self.duration_ms.load(Ordering::Relaxed),
            volume: f32::from_bits(self.volume.load(Ordering::Relaxed)),
            shuffle: snap.shuffle,
            repeat_mode: snap.repeat,
            queue_index: snap.index,
            current,
            queue: snap.queue.clone(),
            error: self.error.lock().clone(),
            warning: joined_notice(self),
            output_label: self.output_label.lock().clone(),
        }
    }
}

fn joined_notice(status: &Status) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(warning) = status.warning.lock().clone() {
        parts.push(warning);
    }
    if let Some(notice) = status.format_notice.lock().clone() {
        parts.push(notice);
    }
    if status.exclusive.load(Ordering::Relaxed) && !status.lock_volume.load(Ordering::Relaxed) {
        let volume = f32::from_bits(status.volume.load(Ordering::Relaxed));
        if volume < 0.999 {
            let mut text = "独占模式的音量是数字音量，低于 100% 时不是位完美".to_string();
            if status.dop.load(Ordering::Relaxed) {
                text.push_str("，DoP 标记也会被改变");
            }
            parts.push(text);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("。"))
    }
}

fn position_ms(status: &Status) -> u64 {
    let rate = u64::from(status.out_rate.load(Ordering::Relaxed).max(1));
    let frames = status.frames.load(Ordering::Relaxed);
    let start = status.track_start_frame.load(Ordering::Relaxed);
    let origin = status.origin_ms.load(Ordering::Relaxed);
    let pos = origin.saturating_add(frames.saturating_sub(start).saturating_mul(1000) / rate);
    let duration = status.duration_ms.load(Ordering::Relaxed);
    if duration > 0 {
        pos.min(duration)
    } else {
        pos
    }
}

fn audio_thread(rx: Receiver<Envelope>, status: Arc<Status>, ffmpeg: Option<PathBuf>) {
    let output = OutputSession::start(Arc::clone(&status));
    let out_rate = status.out_rate.load(Ordering::Relaxed).max(1);
    let resampler = StereoResampler::new(out_rate, out_rate).unwrap_or_else(|_| {
        StereoResampler::new(48_000, 48_000).expect("passthrough resampler")
    });
    let mut player = Player {
        status,
        ffmpeg,
        decoder: None,
        input_rate: out_rate,
        resampler,
        preloaded: None,
        preload_for: None,
        pending: None,
        skips_left: SKIP_BUDGET,
        output,
        prefs: OutputPrefs::default(),
    };
    loop {
        loop {
            match rx.try_recv() {
                Ok(envelope) => {
                    player.handle(envelope.cmd);
                    let _ = envelope.ack.send(());
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Some(message) = player.status.take_fault() {
            player.on_device_fault(message);
        }
        player.apply_pending();
        if player.buffer_full() {
            player.maybe_preload();
            thread::sleep(Duration::from_millis(8));
            continue;
        }
        if player.decoder.is_none() {
            thread::sleep(Duration::from_millis(12));
            continue;
        }
        let pulled = player.decoder.as_mut().and_then(|decoder| match decoder.pull() {
            Ok(value) => Some(Ok(value)),
            Err(err) => Some(Err(err)),
        });
        if let Some(decoder) = player.decoder.as_ref() {
            player.adopt_rate(decoder.rate());
        }
        match pulled {
            Some(Ok(Some(pcm))) => {
                let converted = player.resampler.process(&pcm);
                player.append(&converted);
            }
            Some(Ok(None)) => player.on_eof(),
            Some(Err(err)) => {
                player.set_error(err);
                player.decoder = None;
                player.skips_left = player.skips_left.saturating_sub(1);
                player.on_eof();
            }
            None => thread::sleep(Duration::from_millis(12)),
        }
    }
}

impl Player {
    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::SetQueue { tracks, index } => self.set_queue(tracks, index),
            Cmd::Toggle => self.toggle(),
            Cmd::Play => self.play(),
            Cmd::Pause => {
                self.status.playing.store(false, Ordering::Relaxed);
            }
            Cmd::Next => self.skip_next(),
            Cmd::Prev => self.skip_prev(),
            Cmd::Seek(ms) => self.seek_to(ms),
            Cmd::Volume(volume) => {
                let volume = volume.clamp(0.0, 1.0);
                self.status.volume.store(volume.to_bits(), Ordering::Relaxed);
            }
            Cmd::Shuffle(enabled) => {
                let mut snap = self.status.snap.lock();
                snap.shuffle = enabled;
                let len = snap.queue.len();
                let current = snap.index;
                snap.order = rebuild_order(len, enabled, current);
                drop(snap);
                self.preloaded = None;
                self.preload_for = None;
            }
            Cmd::Repeat(mode) => {
                self.status.snap.lock().repeat = mode;
                self.preloaded = None;
                self.preload_for = None;
            }
            Cmd::Enqueue(tracks) => self.enqueue(tracks),
            Cmd::PlayNext(track) => self.play_next(track),
            Cmd::Jump(index) => {
                self.skips_left = SKIP_BUDGET;
                self.open_index(index, false, 0);
                self.status.playing.store(true, Ordering::Relaxed);
            }
            Cmd::Remove(index) => self.remove_at(index),
            Cmd::Clear => self.clear_queue(),
            Cmd::Output(prefs) => self.configure_output(prefs),
            Cmd::UpdateArt { album, artist, art_path } => {
                let mut snap = self.status.snap.lock();
                for track in &mut snap.queue {
                    let same_album = track.album == album;
                    let same_artist = track.artist == artist || track.album_artist == artist;
                    if same_album && same_artist && !track.has_art {
                        track.has_art = true;
                        track.art_path = Some(art_path.clone());
                    }
                }
            }
        }
    }

    fn set_queue(&mut self, tracks: Vec<Track>, index: usize) {
        if tracks.is_empty() {
            self.clear_queue();
            return;
        }
        let index = index.min(tracks.len() - 1);
        {
            let mut snap = self.status.snap.lock();
            let shuffle = snap.shuffle;
            snap.order = rebuild_order(tracks.len(), shuffle, Some(index));
            snap.queue = tracks;
            snap.index = Some(index);
        }
        self.skips_left = SKIP_BUDGET;
        self.open_index(index, false, 0);
        self.status.playing.store(true, Ordering::Relaxed);
    }

    fn toggle(&mut self) {
        let playing = self.status.playing.load(Ordering::Relaxed);
        if playing {
            self.status.playing.store(false, Ordering::Relaxed);
            return;
        }
        self.play();
    }

    fn play(&mut self) {
        if self.decoder.is_none() {
            let index = self.status.snap.lock().index;
            if let Some(index) = index {
                let restart = position_ms(&self.status) + 200
                    >= self.status.duration_ms.load(Ordering::Relaxed)
                    && self.status.duration_ms.load(Ordering::Relaxed) > 0;
                self.skips_left = SKIP_BUDGET;
                self.open_index(index, false, if restart { 0 } else { position_ms(&self.status) });
            }
        }
        if self.decoder.is_some() {
            self.status.playing.store(true, Ordering::Relaxed);
        }
    }

    fn skip_next(&mut self) {
        let next = {
            let snap = self.status.snap.lock();
            next_queue_index(&snap, false)
        };
        if let Some(index) = next {
            self.skips_left = SKIP_BUDGET;
            self.open_index(index, false, 0);
            self.status.playing.store(true, Ordering::Relaxed);
        }
    }

    fn skip_prev(&mut self) {
        if position_ms(&self.status) > 3_000 {
            self.seek_to(0);
            return;
        }
        let prev = {
            let snap = self.status.snap.lock();
            prev_queue_index(&snap)
        };
        if let Some(index) = prev {
            self.skips_left = SKIP_BUDGET;
            self.open_index(index, false, 0);
            self.status.playing.store(true, Ordering::Relaxed);
        }
    }

    fn seek_to(&mut self, mut position_ms: u64) {
        let duration = self.status.duration_ms.load(Ordering::Relaxed);
        if duration > 0 {
            position_ms = position_ms.min(duration.saturating_sub(1));
        }
        let index = self.status.snap.lock().index;
        if let Some(index) = index {
            self.skips_left = SKIP_BUDGET;
            self.open_index(index, false, position_ms);
        }
    }

    fn enqueue(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let mut snap = self.status.snap.lock();
        let start = snap.queue.len();
        snap.queue.extend(tracks);
        let len = snap.queue.len();
        if snap.shuffle {
            snap.order.extend(start..len);
        } else {
            snap.order = (0..snap.queue.len()).collect();
        }
        if snap.index.is_none() {
            snap.index = Some(0);
        }
    }

    fn play_next(&mut self, track: Track) {
        let mut snap = self.status.snap.lock();
        let insert_at = snap.index.map(|index| index + 1).unwrap_or(0);
        snap.queue.insert(insert_at, track);
        let current = snap.index;
        let shuffle = snap.shuffle;
        let len = snap.queue.len();
        snap.order = rebuild_order(len, shuffle, current);
        if snap.index.is_none() {
            snap.index = Some(0);
        }
        drop(snap);
        self.preloaded = None;
        self.preload_for = None;
    }

    fn remove_at(&mut self, index: usize) {
        let removing_current = {
            let mut snap = self.status.snap.lock();
            if index >= snap.queue.len() {
                return;
            }
            let removing_current = snap.index == Some(index);
            snap.queue.remove(index);
            snap.index = match snap.index {
                Some(current) if index < current => Some(current - 1),
                Some(current) if index == current => {
                    if snap.queue.is_empty() {
                        None
                    } else {
                        Some(index.min(snap.queue.len() - 1))
                    }
                }
                other => other,
            };
            let current = snap.index;
            let shuffle = snap.shuffle;
            let len = snap.queue.len();
            snap.order = rebuild_order(len, shuffle, current);
            removing_current
        };
        self.preloaded = None;
        self.preload_for = None;
        if removing_current {
            let next = self.status.snap.lock().index;
            if let Some(next) = next {
                self.skips_left = SKIP_BUDGET;
                self.open_index(next, false, 0);
            } else {
                self.stop_decoder();
            }
        }
    }

    fn clear_queue(&mut self) {
        let mut snap = self.status.snap.lock();
        snap.queue.clear();
        snap.index = None;
        snap.order.clear();
        drop(snap);
        self.stop_decoder();
    }

    fn stop_decoder(&mut self) {
        self.decoder = None;
        self.pending = None;
        self.preloaded = None;
        self.preload_for = None;
        self.status.buf.lock().clear();
        self.resampler.reset();
        self.status.playing.store(false, Ordering::Relaxed);
        self.status.frames.store(0, Ordering::Relaxed);
        self.status.origin_ms.store(0, Ordering::Relaxed);
        self.status.track_start_frame.store(0, Ordering::Relaxed);
        self.status.duration_ms.store(0, Ordering::Relaxed);
    }

    fn open_index(&mut self, index: usize, gapless: bool, offset_ms: u64) {
        if self.skips_left == 0 {
            self.set_error("连续多首无法播放，已停止".into());
            self.stop_decoder();
            return;
        }
        let track = {
            let snap = self.status.snap.lock();
            snap.queue.get(index).cloned()
        };
        let Some(track) = track else {
            self.stop_decoder();
            return;
        };
        let spec = negotiate::source_from_track(&track, self.prefs.exclusive, self.prefs.dop);
        if gapless && !self.output.satisfies(&spec, &self.prefs) {
            self.open_index(index, false, offset_ms);
            return;
        }
        let mut opened = self.output.realign(&spec, &self.prefs);
        let source = if opened.dop {
            match dop::open_dop(&track) {
                Ok(source) => source,
                Err(err) => {
                    let mut pcm = spec.clone();
                    pcm.want_dop = false;
                    opened = self.output.realign(&pcm, &self.prefs);
                    *self.status.format_notice.lock() =
                        Some(format!("无法按 DoP 读取：{err}。已改为 PCM"));
                    match open_source(&track, self.ffmpeg.as_deref(), offset_ms) {
                        Ok(source) => source,
                        Err(err) => {
                            self.fail_open(&track, index, err);
                            return;
                        }
                    }
                }
            }
        } else {
            match open_source(&track, self.ffmpeg.as_deref(), offset_ms) {
                Ok(source) => source,
                Err(err) => {
                    self.fail_open(&track, index, err);
                    return;
                }
            }
        };
        self.skips_left = SKIP_BUDGET;
        self.clear_error();
        self.adopt_output(&opened);
        self.install(source, index, gapless, offset_ms, track.duration_ms);
    }

    fn fail_open(&mut self, track: &Track, index: usize, err: String) {
        self.set_error(format!("无法播放「{}」：{err}", track.title));
        self.skips_left = self.skips_left.saturating_sub(1);
        self.decoder = None;
        self.preloaded = None;
        self.preload_for = None;
        let follow = {
            let mut snap = self.status.snap.lock();
            snap.index = Some(index);
            next_queue_index(&snap, false)
        };
        if let Some(follow) = follow {
            if follow != index {
                self.open_index(follow, false, 0);
            }
        } else {
            self.status.playing.store(false, Ordering::Relaxed);
        }
    }

    fn configure_output(&mut self, prefs: OutputPrefs) {
        let changed = self.prefs.device_id != prefs.device_id
            || self.prefs.exclusive != prefs.exclusive
            || self.prefs.dop != prefs.dop;
        self.prefs = prefs.clone();
        self.status
            .lock_volume
            .store(prefs.lock_volume, Ordering::Relaxed);
        if !changed {
            return;
        }
        self.output.clear_block();
        self.preloaded = None;
        self.preload_for = None;
        let index = self.status.snap.lock().index;
        if let Some(index) = index {
            if self.decoder.is_some() {
                let position = position_ms(&self.status);
                self.open_index(index, false, position);
            }
        }
    }

    fn on_device_fault(&mut self, message: String) {
        let _opened = self.output.force_shared(message);
        self.preloaded = None;
        self.preload_for = None;
        let index = self.status.snap.lock().index;
        if let Some(index) = index {
            let position = position_ms(&self.status);
            self.open_index(index, false, position);
        }
    }

    fn adopt_output(&mut self, opened: &Opened) {
        if opened.rate == self.resampler.output_rate() {
            return;
        }
        match self.resampler.set_output_rate(opened.rate) {
            Ok(flushed) => self.append(&flushed),
            Err(err) => self.set_error(err),
        }
    }

    fn install(
        &mut self,
        source: Box<dyn PcmSource>,
        index: usize,
        gapless: bool,
        offset_ms: u64,
        duration_ms: u64,
    ) {
        let rate = source.rate().max(1);
        if gapless {
            self.adopt_rate(rate);
            let at_frame = self.status.frames.load(Ordering::Relaxed) + self.buffered_frames();
            self.pending = Some(PendingSwitch {
                at_frame,
                index,
                duration_ms,
            });
        } else {
            self.status.buf.lock().clear();
            self.resampler.reset();
            self.input_rate = 0;
            self.adopt_rate(rate);
            self.pending = None;
            self.preloaded = None;
            self.preload_for = None;
            self.status.frames.store(0, Ordering::Relaxed);
            self.status.track_start_frame.store(0, Ordering::Relaxed);
            self.status.origin_ms.store(offset_ms, Ordering::Relaxed);
            self.status.duration_ms.store(duration_ms, Ordering::Relaxed);
            self.status.snap.lock().index = Some(index);
        }
        self.decoder = Some(source);
        self.input_rate = rate;
    }

    fn adopt_rate(&mut self, rate: u32) {
        let rate = rate.max(1);
        if rate == self.input_rate {
            return;
        }
        match self.resampler.set_input_rate(rate) {
            Ok(flushed) => self.append(&flushed),
            Err(err) => self.set_error(err),
        }
        self.input_rate = rate;
    }

    fn on_eof(&mut self) {
        let action = {
            let snap = self.status.snap.lock();
            if snap.repeat == RepeatMode::One {
                EofAction::Restart(snap.index)
            } else {
                match next_queue_index(&snap, true) {
                    Some(index) => EofAction::Advance(index),
                    None => EofAction::Stop,
                }
            }
        };
        match action {
            EofAction::Restart(Some(index)) => {
                self.skips_left = SKIP_BUDGET;
                self.open_index(index, false, 0);
            }
            EofAction::Advance(index) => {
                let track = self.status.snap.lock().queue.get(index).cloned();
                if let Some(track) = track.as_ref() {
                    let spec = negotiate::source_from_track(&track, self.prefs.exclusive, self.prefs.dop);
                    let dop_track = spec.want_dop;
                    if dop_track || !self.output.satisfies(&spec, &self.prefs) {
                        self.open_index(index, false, 0);
                        return;
                    }
                }
                let wanted = self.status.snap.lock().queue.get(index).map(|track| track.id);
                if let (Some(wanted), Some((id, source))) = (wanted, self.preloaded.take()) {
                    if id == wanted {
                        let duration = self
                            .status
                            .snap
                            .lock()
                            .queue
                            .get(index)
                            .map(|track| track.duration_ms)
                            .unwrap_or(0);
                        self.install(source, index, true, 0, duration);
                        self.preload_for = None;
                        return;
                    }
                }
                self.preload_for = None;
                self.open_index(index, true, 0);
            }
            EofAction::Stop | EofAction::Restart(None) => {
                self.decoder = None;
                self.pending = None;
                self.status.playing.store(false, Ordering::Relaxed);
            }
        }
    }

    fn apply_pending(&mut self) {
        let ready = self
            .pending
            .as_ref()
            .is_some_and(|pending| self.status.frames.load(Ordering::Relaxed) >= pending.at_frame);
        if !ready {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.status
            .track_start_frame
            .store(pending.at_frame, Ordering::Relaxed);
        self.status.origin_ms.store(0, Ordering::Relaxed);
        self.status
            .duration_ms
            .store(pending.duration_ms, Ordering::Relaxed);
        let mut snap = self.status.snap.lock();
        if pending.index < snap.queue.len() {
            snap.index = Some(pending.index);
        }
    }

    fn maybe_preload(&mut self) {
        if self.decoder.is_none() || self.pending.is_some() {
            return;
        }
        let track = {
            let snap = self.status.snap.lock();
            if snap.repeat == RepeatMode::One {
                return;
            }
            let Some(index) = next_queue_index(&snap, true) else {
                return;
            };
            snap.queue.get(index).cloned()
        };
        let Some(track) = track else {
            return;
        };
        if self.preload_for == Some(track.id) {
            return;
        }
        let spec = negotiate::source_from_track(&track, self.prefs.exclusive, self.prefs.dop);
        if spec.want_dop || !self.output.satisfies(&spec, &self.prefs) {
            return;
        }
        let remain = self
            .status
            .duration_ms
            .load(Ordering::Relaxed)
            .saturating_sub(position_ms(&self.status));
        if remain > 12_000 {
            return;
        }
        self.preload_for = Some(track.id);
        match open_source(&track, self.ffmpeg.as_deref(), 0) {
            Ok(source) => self.preloaded = Some((track.id, source)),
            Err(_) => self.preloaded = None,
        }
    }

    fn buffer_full(&self) -> bool {
        let rate = self.status.out_rate.load(Ordering::Relaxed).max(1) as u64;
        let target = rate.saturating_mul(4) / 5;
        self.buffered_frames() >= target.max(1)
    }

    fn buffered_frames(&self) -> u64 {
        (self.status.buf.lock().len() / 2) as u64
    }

    fn append(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        self.status.buf.lock().extend(samples.iter().copied());
    }

    fn set_error(&self, message: String) {
        *self.status.error.lock() = Some(message);
    }

    fn clear_error(&self) {
        *self.status.error.lock() = None;
    }
}

enum EofAction {
    Advance(usize),
    Restart(Option<usize>),
    Stop,
}

fn next_queue_index(snap: &Snap, natural: bool) -> Option<usize> {
    let current = snap.index?;
    if snap.queue.is_empty() {
        return None;
    }
    if snap.shuffle {
        let pos = snap
            .order
            .iter()
            .position(|index| *index == current)
            .unwrap_or(0);
        if pos + 1 < snap.order.len() {
            return Some(snap.order[pos + 1]);
        }
        if snap.repeat == RepeatMode::All || !natural && snap.repeat == RepeatMode::One {
            return snap.order.first().copied();
        }
        if !natural && snap.repeat == RepeatMode::Off {
            return None;
        }
        return if snap.repeat == RepeatMode::All {
            snap.order.first().copied()
        } else {
            None
        };
    }
    if current + 1 < snap.queue.len() {
        return Some(current + 1);
    }
    if snap.repeat == RepeatMode::All {
        Some(0)
    } else {
        None
    }
}

fn prev_queue_index(snap: &Snap) -> Option<usize> {
    let current = snap.index?;
    if snap.queue.is_empty() {
        return None;
    }
    if snap.shuffle {
        let pos = snap
            .order
            .iter()
            .position(|index| *index == current)
            .unwrap_or(0);
        if pos > 0 {
            return Some(snap.order[pos - 1]);
        }
        return if snap.repeat == RepeatMode::All {
            snap.order.last().copied()
        } else {
            Some(current)
        };
    }
    if current > 0 {
        return Some(current - 1);
    }
    if snap.repeat == RepeatMode::All {
        Some(snap.queue.len() - 1)
    } else {
        Some(0)
    }
}

fn rebuild_order(len: usize, shuffle: bool, current: Option<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    if !shuffle || len < 2 {
        return order;
    }
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0x00C0_FFEE);
    for index in (1..len).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let swap = (state as usize) % (index + 1);
        order.swap(index, swap);
    }
    if let Some(current) = current {
        if let Some(pos) = order.iter().position(|index| *index == current) {
            order.swap(0, pos);
        }
    }
    order
}

fn f32_callback(
    status: Arc<Status>,
    channels: usize,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut smoothed = f32::from_bits(status.volume.load(Ordering::Relaxed));
    move |data, _info| {
        let frames = if channels == 0 { 0 } else { data.len() / channels };
        let pairs = pull_stereo(&status, &mut smoothed, frames);
        for (frame, sample) in data.chunks_mut(channels.max(1)).enumerate() {
            let (left, right) = pairs.get(frame).copied().unwrap_or((0.0, 0.0));
            write_frame_f32(sample, left, right);
        }
    }
}

fn i16_callback(
    status: Arc<Status>,
    channels: usize,
) -> impl FnMut(&mut [i16], &cpal::OutputCallbackInfo) + Send + 'static {
    let mut smoothed = f32::from_bits(status.volume.load(Ordering::Relaxed));
    move |data, _info| {
        let frames = if channels == 0 { 0 } else { data.len() / channels };
        let pairs = pull_stereo(&status, &mut smoothed, frames);
        for (frame, sample) in data.chunks_mut(channels.max(1)).enumerate() {
            let (left, right) = pairs.get(frame).copied().unwrap_or((0.0, 0.0));
            write_frame_i16(sample, left, right);
        }
    }
}

fn pull_stereo(status: &Status, smoothed: &mut f32, frames: usize) -> Vec<(f32, f32)> {
    let exclusive = status.exclusive.load(Ordering::Relaxed);
    let locked = exclusive && status.lock_volume.load(Ordering::Relaxed);
    let volume = f32::from_bits(status.volume.load(Ordering::Relaxed)).clamp(0.0, 1.0);
    let target = if locked { 1.0 } else { volume };
    if exclusive && (locked || (target - 1.0).abs() < 0.001) {
        *smoothed = 1.0;
    } else {
        *smoothed += (target - *smoothed) * 0.2;
    }
    if frames == 0 || !status.playing.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(frames);
    let mut consumed = 0u64;
    {
        let mut buf = status.buf.lock();
        for _ in 0..frames {
            if buf.len() < 2 {
                break;
            }
            let left = buf.pop_front().unwrap_or(0.0) * *smoothed;
            let right = buf.pop_front().unwrap_or(0.0) * *smoothed;
            out.push((left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0)));
            consumed += 1;
        }
    }
    status.frames.fetch_add(consumed, Ordering::Relaxed);
    out
}

fn write_frame_f32(sample: &mut [f32], left: f32, right: f32) {
    match sample.len() {
        0 => {}
        1 => sample[0] = (left + right) * 0.5,
        _ => {
            sample[0] = left;
            sample[1] = right;
            for extra in &mut sample[2..] {
                *extra = 0.0;
            }
        }
    }
}

fn write_frame_i16(sample: &mut [i16], left: f32, right: f32) {
    let convert = |value: f32| (value.clamp(-1.0, 1.0) * 32767.0) as i16;
    match sample.len() {
        0 => {}
        1 => sample[0] = convert((left + right) * 0.5),
        _ => {
            sample[0] = convert(left);
            sample[1] = convert(right);
            for extra in &mut sample[2..] {
                *extra = 0;
            }
        }
    }
}

#[cfg(test)]
mod playback_tests {
    use super::*;

    #[test]
    fn queued_wav_moves_the_playhead() {
        let dir = std::env::temp_dir().join(format!("chengyin-play-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..44_100 {
            let sample = ((index as f32) * 0.01).sin();
            writer.write_sample((sample * 8000.0) as i16).unwrap();
            writer.write_sample((sample * 8000.0) as i16).unwrap();
        }
        writer.finalize().unwrap();
        let track = Track {
            id: 7,
            path: path.to_string_lossy().to_string(),
            cue_path: None,
            start_ms: 0,
            end_ms: None,
            title: "试音".into(),
            artist: "澄音".into(),
            album_artist: String::new(),
            album: "试音".into(),
            track_number: Some(1),
            disc_number: None,
            year: String::new(),
            duration_ms: 1000,
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            format: "WAV".into(),
            folder: dir.to_string_lossy().to_string(),
            has_art: false,
            art_path: None,
        };
        let engine = AudioHandle::start(None);
        engine.set_queue(vec![track], 0).expect("engine accepted the queue");
        thread::sleep(Duration::from_millis(350));
        let state = engine.snapshot();
        assert!(
            state.position_ms > 0,
            "position did not move: {state:?}"
        );
        engine.pause().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn null_clock(status: Arc<Status>, stop: Arc<AtomicBool>) {
    let mut last = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let elapsed = last.elapsed();
        last = Instant::now();
        let rate = status.out_rate.load(Ordering::Relaxed).max(1) as f64;
        let frames = (elapsed.as_secs_f64() * rate) as usize;
        if status.playing.load(Ordering::Relaxed) && frames > 0 {
            let mut consumed = 0u64;
            {
                let mut buf = status.buf.lock();
                for _ in 0..frames {
                    if buf.len() < 2 {
                        break;
                    }
                    buf.pop_front();
                    buf.pop_front();
                    consumed += 1;
                }
            }
            status.frames.fetch_add(consumed, Ordering::Relaxed);
        }
        thread::sleep(Duration::from_millis(10));
    }
}
