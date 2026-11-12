use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tracing::{debug, error, warn};
use water_ball_tool_lib::music_play::LoopMode;
use water_ball_tool_lib::music_play::decoder_runtime::{
    DecoderError, DecoderErrorKind, DecoderEventChannelSink, DecoderEventDirectSink,
    DecoderEventType,
};
use water_ball_tool_lib::music_play::effect::EffectChainKind;
use water_ball_tool_lib::music_play::effect::EffectSummary;
use water_ball_tool_lib::music_play::effect::fade::FadeEffect;
use water_ball_tool_lib::music_play::effect::runtime::PreEffectRuntimeEventChannelSink;
use water_ball_tool_lib::music_play::play_list::{PlayList, Track};
use water_ball_tool_lib::music_play::player::{Player, PlayerHandle};
use water_ball_tool_lib::music_play::tools::DurationStatsWindow;

// 打开曲目失败的重试退避与连续失败上限 / retry backoff and consecutive failure limit for opening a track
const RETRY_BACKOFF: Duration = Duration::from_millis(200);
const MAX_START_ERRORS: usize = 5;

/// 播放运行时：TUI 主循环直接调用，事件回调只通过 `Shared` 交换状态。
///
/// Playback runtime: the TUI loop calls it directly; event callbacks only exchange state through `Shared`.
pub(super) struct Runtime {
    player: Player,
    shared: Arc<Shared>,
    // 最近一次非空的耗时窗口缓存：统计按块刷新，50ms 采样时大多数窗口为空，不能直接清零
    // Cache of the last non-empty timing windows: stats flush per block, so most 50ms samples are empty.
    debug_metrics: Mutex<Vec<(&'static str, DurationStatsWindow)>>,
    //AI=== 效果器窗口打开时缓存最后一次成功快照，避免 try_lock 失败导致列表闪烁
    // Cache the last successful effects snapshot so a failed try_lock does not flicker the list.
    effect_cache: Mutex<Vec<EffectSummary>>,
    //AI_END===
}

/// 事件回调与 UI 线程共享的状态 / state shared by event callbacks and the UI thread
struct Shared {
    play_list: Mutex<PlayList>,
    /// 换曲 ID：传给 `PlayList::next`，避免同一轮 Eof 被处理两次
    ///
    /// Track-change id passed to `PlayList::next`, so one Eof cannot advance twice.
    next_id: AtomicUsize,
    /// 连续打开曲目失败次数 / consecutive track-open failures
    error_streak: AtomicUsize,
    /// 换曲互斥：手动换曲与自动 Eof 同时发生时只放行一个
    ///
    /// Switch guard: only one of manual next / auto Eof runs at a time.
    switching: AtomicBool,
    /// 停止标志：让进行中的换曲尽快退出 / stop flag so an in-flight track change exits promptly
    stop: AtomicBool,
}

impl Shared {
    fn new(loop_mode: LoopMode) -> Self {
        let mut play_list = PlayList::new();
        play_list.set_loop_mod(loop_mode);
        Self {
            play_list: Mutex::new(play_list),
            next_id: AtomicUsize::new(0),
            error_streak: AtomicUsize::new(0),
            switching: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        }
    }
}

/// UI 每帧读取的播放快照 / playback snapshot read by the UI every frame
pub(super) struct Snapshot {
    pub(super) track: Option<Track>,
    pub(super) pos: Duration,
    pub(super) volume: f32,
    pub(super) playing: bool,
    pub(super) poisoned: bool,
    pub(super) mode: LoopMode,
    pub(super) index: Option<usize>,
    pub(super) debug: DebugInfo,
    //AI=== 效果器窗口打开时才有值 / present only while the effects window is open
    pub(super) effects: Option<Vec<EffectSummary>>,
    //AI_END===
}

/// Debug 弹窗的数据：缓冲区、链路延迟、解码器状态与各段耗时 / debug popup data: buffers, latency, decoder state and per-stage timings
#[derive(Default)]
pub(super) struct DebugInfo {
    pub(super) decoder_path: Option<String>,
    pub(super) decoder_format: Option<(u32, u16)>,
    //AI=== 播放格式：输出设备实际采样率与声道 / playback format: the device's actual rate and channels
    pub(super) playback_format: Option<(u32, u16)>,
    //AI_END===
    pub(super) pre_effect_len: usize,
    pub(super) pre_effect_capacity: usize,
    pub(super) audio_len: usize,
    //AI=== 与 pre 统一按“已用 / 容量”展示 / show occupancy over capacity like `pre`
    pub(super) audio_capacity: usize,
    //AI_END===
    pub(super) playlist_len: usize,
    pub(super) playlist_index: Option<usize>,
    pub(super) pre_effect_eof: bool,
    pub(super) lock_fail: u64,
    pub(super) underrun: u64,
    pub(super) metrics: Vec<(&'static str, DurationStatsWindow)>,
    //AI=== 链路延迟估算 / buffer-based latency estimates
    pub(super) decoder_buffer_latency: Duration,
    pub(super) pre_effect_queue_latency: Duration,
    pub(super) effect_latency: Duration,
    pub(super) playback_latency: Duration,
    pub(super) total_latency: Duration,
    //AI_END===
}

impl Runtime {
    pub(super) fn new(
        loop_mode: LoopMode,
        volume: f32,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut player = Player::new()?;
        player.set_volume(volume);
        let shared = Arc::new(Shared::new(loop_mode));

        // 自动换曲在解码线程上执行：此时该曲已无解码任务，可直接切到下一首
        // Auto advance runs on the decode thread: the track has no decode work left, so it can switch directly.
        {
            let this = player.clone();
            let shared = shared.clone();
            player
                .decoder_runtime_mut()
                .add_event(Box::new(DecoderEventDirectSink::create(move |event| {
                    if matches!(event.result(), Ok(DecoderEventType::Eof))
                        && let Err(e) = advance_inner(&this, &shared, true, Advance::Next)
                    {
                        error!("自动换曲失败 / auto advance failed: {e}");
                    }
                })));
        }

        // 解码错误：暂停并让 UI 显示毒化状态；换曲由用户在 Play 时触发
        // Decode errors: pause and let the UI show the poisoned state; the user triggers the track change via Play.
        {
            let this = player.clone();
            player
                .decoder_runtime_mut()
                .add_event(Box::new(DecoderEventChannelSink::create(move |event| {
                    if let Err(e) = event.result() {
                        error!("解码错误，暂停播放 / decode error, pausing: {e:?}");
                        if let Err(e) = this.pause() {
                            error!("暂停失败 / failed to pause: {e}");
                        }
                    }
                })?));
        }

        player.pre_effect_runtime_mut().add_event(Box::new(
            PreEffectRuntimeEventChannelSink::create(|event| {
                if event.result().is_err() {
                    error!("前效果器运行期错误 / pre-effect runtime error: {event:?}");
                }
            })?,
        ));

        player.add_audio_post_effect(FadeEffect::new());

        Ok(Self {
            player,
            shared,
            debug_metrics: Mutex::new(Vec::new()),
            //AI=== 效果器列表初始为空 / the effect list starts empty
            effect_cache: Mutex::new(Vec::new()),
            //AI_END===
        })
    }

    /// 追加曲目 / append a track
    pub(super) fn add_track(&self, track: Track) {
        let mut list = self
            .shared
            .play_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        list.add_track(track);
    }

    /// 播放；解码器毒化时先换曲，尚未加载曲目时先取第一首
    ///
    /// Play; while poisoned, change the track first, and load the first track when none is loaded yet.
    pub(super) fn play(&self) -> Result<(), Box<dyn Error>> {
        if self.player.decoder_runtime().is_poisoned() {
            debug!("解码器毒化，先换曲再播放 / decoder poisoned, changing track before playing");
            self.advance(false)?;
        }

        let (has_tracks, has_current) = {
            let list = self
                .shared
                .play_list
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (list.len() > 0, list.get_current_track().is_some())
        };
        if !has_tracks {
            debug!("播放列表为空，忽略播放 / empty playlist, ignoring play");
            return Ok(());
        }
        if !has_current {
            self.advance(false)?;
        }

        self.player.play()?;
        Ok(())
    }

    pub(super) fn pause(&self) -> Result<(), Box<dyn Error>> {
        self.player.pause()?;
        Ok(())
    }

    pub(super) fn is_playing(&self) -> bool {
        self.player.is_playing()
    }

    //AI=== 启动初始化需要知道当前是否已加载曲目以及循环模式 / startup needs the loaded state and loop mode
    pub(super) fn has_current_track(&self) -> bool {
        self.shared
            .play_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_current_track()
            .is_some()
    }

    pub(super) fn loop_mode(&self) -> LoopMode {
        self.shared
            .play_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .loop_mod()
    }
    //AI_END===

    /// 手动下一曲 / manual next track
    pub(super) fn next_track(&self) -> Result<(), Box<dyn Error>> {
        self.advance(false).map(|_| ())
    }

    //AI=== 命令模式：上一曲与循环模式切换 / command mode: previous track and loop-mode switch
    /// 手动上一曲；轨迹起点处忽略 / manual previous track; ignored at the start of the history
    pub(super) fn prev_track(&self) -> Result<(), Box<dyn Error>> {
        advance_inner(&self.player, &self.shared, false, Advance::Prev).map(|_| ())
    }

    /// 切换循环模式 / switch the loop mode
    pub(super) fn set_loop_mode(&self, mode: LoopMode) {
        self.shared
            .play_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_loop_mod(mode);
    }
    //AI_END===

    //AI=== 命令模式 seek：换曲中忽略，目标按当前曲目时长夹取 / command-mode seek: ignored while switching, clamped to the current duration
    pub(super) fn seek(&self, target: Duration) -> Result<(), Box<dyn Error>> {
        if self.shared.switching.load(Ordering::Acquire) {
            debug!("换曲进行中，忽略 seek / track change in progress, ignoring seek");
            return Ok(());
        }
        //AI=== 曲目收尾/交叉渐变期间 pos 仍包含旧曲尾部，此时跳转会落到新曲的错误位置
        // During the tail/crossfade window `pos` still includes the old track, so a seek would land at the wrong
        // position in the new track; ignore it until the tail drains.
        if self.player.get_pre_effect_item_eof() {
            debug!(
                "曲目收尾/交叉渐变中，忽略 seek / track tail or crossfade in progress, ignoring seek"
            );
            return Ok(());
        }
        //AI_END===
        let target = {
            let list = self
                .shared
                .play_list
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match list.get_current_track() {
                Some(track) if !track.duration().is_zero() => target.min(*track.duration()),
                _ => target,
            }
        };
        self.player.try_seek(target)
    }
    //AI_END===

    //AI=== 点击播放列表：切到指定曲目并开始播放 / click on the playlist: switch to that track and play it
    pub(super) fn play_at(&self, index: usize) -> Result<(), Box<dyn Error>> {
        if self.advance_to(index)?.is_some() {
            self.player.play()?;
        }
        Ok(())
    }

    fn advance_to(&self, index: usize) -> Result<Option<Track>, Box<dyn Error>> {
        advance_inner(&self.player, &self.shared, false, Advance::Jump(index))
    }
    //AI_END===

    pub(super) fn add_volume(&self, delta: f32) {
        self.player.set_volume(self.player.volume() + delta);
    }

    //AI=== 效果器启用/禁用 / enable or disable an effect
    /// 设置指定效果器的启用状态；索引无效时返回 false
    /// Set the enabled state of an effect; returns false when the index is invalid.
    pub(super) fn set_effect_enabled(
        &self,
        chain: EffectChainKind,
        index: usize,
        enabled: bool,
    ) -> bool {
        self.player.set_effect_enabled(chain, index, enabled)
    }
    //AI_END===

    pub(super) fn snapshot(&self, include_effects: bool) -> Snapshot {
        let (track, index, mode, playlist_len) = {
            let list = self
                .shared
                .play_list
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                list.get_current_track().cloned(),
                list.current_index(),
                list.loop_mod(),
                list.len(),
            )
        };

        let metrics = self.player.time_metrics();
        let metrics_snapshot = {
            let mut cached = self
                .debug_metrics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (name, stats) in [
                ("decode", &metrics.decode),
                ("decode_write_wait", &metrics.decode_write_wait),
                ("effect_read_wait", &metrics.effect_read_wait),
                ("effect_process", &metrics.effect_process),
                ("effect_write_wait", &metrics.effect_write_wait),
                ("callback_total", &metrics.callback_total),
                ("callback_copy", &metrics.callback_copy),
                ("callback_post_effect", &metrics.callback_post_effect),
            ] {
                if let Some(window) = stats.take() {
                    match cached.iter_mut().find(|(label, _)| *label == name) {
                        Some(entry) => entry.1 = window,
                        None => cached.push((name, window)),
                    }
                }
            }
            cached.clone()
        };

        //AI=== 链路延迟估算：全部按缓冲区占用计算，不含设备/OS 延迟 / latency estimates from buffer occupancy only; device/OS latency excluded
        let decoder_format = self.player.decoder_runtime().decoder_format();
        let output_format = self.player.output_format();
        let playing = self.player.is_playing();
        let decoder_buffered = decoder_format.map_or(Duration::ZERO, |format| {
            format.samples_to_duration(
                metrics.decoder_buffered_samples.load(Ordering::Relaxed) as usize
            )
        });
        let pre_effect_queue = decoder_format.map_or(Duration::ZERO, |format| {
            format.samples_to_duration(
                metrics.pre_effect_queue_samples.load(Ordering::Relaxed) as usize
            )
        });
        let effect_latency = Duration::from_nanos(metrics.effect_delay_ns.load(Ordering::Relaxed));
        let callback_samples = if playing {
            metrics.callback_samples.load(Ordering::Relaxed) as usize
        } else {
            0
        };
        let playback_latency = output_format.samples_to_duration(
            self.player
                .audio_ring_buffer()
                .len()
                .saturating_add(callback_samples),
        );
        let total_latency = decoder_buffered + pre_effect_queue + effect_latency + playback_latency;
        //AI_END===

        //AI=== 仅在效果器窗口打开时获取列表；try_lock 失败保留上次快照 / only fetch while the effects window is open; keep the last snapshot if try_lock fails
        let effects = include_effects.then(|| {
            let mut cache = self
                .effect_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(latest) = self.player.effect_summaries() {
                *cache = latest;
            }
            cache.clone()
        });
        //AI_END===

        let debug = DebugInfo {
            decoder_path: self
                .player
                .decoder_runtime()
                .decoder_path()
                .map(|path| path.display().to_string()),
            decoder_format: decoder_format.map(|format| (format.sample_rate(), format.channels())),
            //AI=== 播放格式：输出设备实际采样率与声道 / playback format: the device's actual rate and channels
            playback_format: Some((output_format.sample_rate(), output_format.channels())),
            //AI_END===
            pre_effect_len: self
                .player
                .pre_effect_runtime()
                .pre_effect_ring_buffer()
                .len(),
            pre_effect_capacity: self
                .player
                .pre_effect_runtime()
                .pre_effect_ring_buffer()
                .capacity(),
            audio_len: self.player.audio_ring_buffer().len(),
            //AI=== 与 pre 统一按“已用 / 容量”展示 / show occupancy over capacity like `pre`
            audio_capacity: self.player.audio_ring_buffer().capacity(),
            //AI_END===
            playlist_len,
            playlist_index: index,
            pre_effect_eof: self.player.get_pre_effect_item_eof(),
            lock_fail: metrics.lock_fail.load(Ordering::Relaxed),
            underrun: metrics.underrun.load(Ordering::Relaxed),
            metrics: metrics_snapshot,
            //AI=== 调试页链路延迟 / debug latency estimates
            decoder_buffer_latency: decoder_buffered,
            pre_effect_queue_latency: pre_effect_queue,
            effect_latency,
            playback_latency,
            total_latency,
            //AI_END===
        };

        Snapshot {
            track,
            pos: self.player.get_pos(),
            volume: self.player.volume(),
            playing,
            poisoned: self.player.decoder_runtime().is_poisoned(),
            mode,
            index,
            debug,
            effects,
        }
    }

    fn advance(&self, auto: bool) -> Result<Option<Track>, Box<dyn Error>> {
        advance_inner(&self.player, &self.shared, auto, Advance::Next)
    }

    /// 停止播放器。幂等；Drop 也会调用 / stop the player; idempotent, also called from Drop
    pub(super) fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Err(e) = self.player.try_stop() {
            error!("停止播放器失败 / failed to stop the player: {e:?}");
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 换曲：选曲 → 打开解码器 → 更新 ID。手动换曲与自动 Eof 共用。
///
/// Change track: pick → open the decoder → update the id. Shared by manual next and auto Eof.
//AI=== target 为点击跳转的 0-based 索引，只在本轮第一次选曲时使用 / target is the clicked 0-based index, used only for the first pick
fn advance_inner(
    player: &PlayerHandle,
    shared: &Shared,
    auto: bool,
    select: Advance,
) -> Result<Option<Track>, Box<dyn Error>> {
    if shared.switching.swap(true, Ordering::AcqRel) {
        debug!(
            "已有换曲在进行，忽略本次请求 / another track change is running, ignoring this request"
        );
        return Ok(None);
    }
    let _switching = SwitchingGuard(&shared.switching);

    if shared.stop.load(Ordering::Acquire) {
        return Ok(None);
    }

    let mut auto = auto;
    // 跳转/回退只在第一次选曲生效，之后的重试按顺序取下一首
    // A jump/step-back applies to the first pick only; retries advance sequentially.
    let mut select = select;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return Ok(None);
        }

        let next = {
            let mut list = shared
                .play_list
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let picked = match std::mem::replace(&mut select, Advance::Next) {
                Advance::Jump(index) => match list.jump_to(index) {
                    Some(next) => Some(next),
                    None => {
                        debug!("跳转索引越界，忽略 / jump index out of range, ignoring");
                        return Ok(None);
                    }
                },
                Advance::Prev => match list.prev() {
                    Some(next) => Some(next),
                    None => {
                        debug!("没有上一曲，忽略 / no previous track, ignoring");
                        return Ok(None);
                    }
                },
                Advance::Next => list.next(auto, shared.next_id.load(Ordering::Acquire)),
            };
            picked.map(|next| (next.next_id(), next.track().clone()))
        };
        let Some((next_id, track)) = next else {
            debug!("没有下一曲，暂停 / no next track, pausing");
            if let Err(e) = player.pause() {
                error!("暂停失败 / failed to pause: {e}");
            }
            if let Err(e) = player.pre_effect_runtime().send_no_next() {
                warn!("发送 NoNext 失败 / failed to send NoNext: {e}");
            }
            return Ok(None);
        };
        shared.next_id.store(next_id, Ordering::Release);

        let mut retries = 0usize;
        loop {
            if shared.stop.load(Ordering::Acquire) {
                return Ok(None);
            }

            // 自动换曲用无缝切换：旧曲尾音留在缓冲区继续播放，新曲排在其后——不等待、不截断
            // Auto advance uses the seamless switch: the old tail stays buffered and keeps playing while the new
            // track is queued behind it — no wait and no truncation.
            let result = if auto && retries == 0 {
                player.set_decoder_path_no_clear(track.path())
            } else {
                // 手动换曲 / 跳过 / 重试：丢弃旧缓冲 / manual change, skip or retry: clear the old buffers
                player.set_decoder_path(track.path())
            };

            match result {
                Ok(()) => {
                    shared.error_streak.store(0, Ordering::Release);
                    debug!(
                        "换曲成功 / track changed: {} - {}",
                        track.title(),
                        track.artist()
                    );
                    return Ok(Some(track));
                }
                Err(e) => {
                    let streak = shared.error_streak.fetch_add(1, Ordering::AcqRel) + 1;
                    error!(
                        "打开曲目失败 ({}) / failed to open track: {e}",
                        track.path().display()
                    );
                    if streak >= MAX_START_ERRORS {
                        if let Err(pause_err) = player.pause() {
                            error!("暂停失败 / failed to pause: {pause_err}");
                        }
                        return Err(Box::new(e));
                    }

                    match classify_start_error(&e) {
                        StartAction::Skip => {
                            // 跳过当前曲目，下一次循环按手动方式取下一首
                            // Skip this track; the next loop iteration advances manually.
                            auto = false;
                            break;
                        }
                        StartAction::Retry => {
                            thread::sleep(RETRY_BACKOFF);
                            retries += 1;
                        }
                        StartAction::Pause => {
                            if let Err(pause_err) = player.pause() {
                                error!("暂停失败 / failed to pause: {pause_err}");
                            }
                            return Err(Box::new(e));
                        }
                    }
                }
            }
        }
    }
}

//AI=== 选曲方式：顺序、回退或跳转 / track selection: sequential, step back or jump
enum Advance {
    Next,
    Prev,
    Jump(usize),
}

/// 换曲互斥的 RAII 复位 / RAII reset for the switch guard
struct SwitchingGuard<'a>(&'a AtomicBool);
impl Drop for SwitchingGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
enum StartAction {
    Skip,
    Retry,
    Pause,
}

/// 按错误类型决定跳过、重试还是暂停 / decide skip, retry or pause from the error kind
fn classify_start_error(error: &DecoderError) -> StartAction {
    use std::io;

    match error.kind() {
        DecoderErrorKind::IoError(e) => match e.kind() {
            io::ErrorKind::NotFound
            | io::ErrorKind::InvalidData
            | io::ErrorKind::Unsupported
            | io::ErrorKind::IsADirectory
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::InvalidInput => StartAction::Skip,
            io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::ResourceBusy
            | io::ErrorKind::StaleNetworkFileHandle
            | io::ErrorKind::NetworkDown
            | io::ErrorKind::NotConnected
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::OutOfMemory
            | io::ErrorKind::StorageFull
            | io::ErrorKind::QuotaExceeded
            | io::ErrorKind::PermissionDenied => StartAction::Retry,
            _ => StartAction::Pause,
        },
        DecoderErrorKind::NoAudioTrack
        | DecoderErrorKind::NoAudioCodecParams
        | DecoderErrorKind::MakeAudioDecoderError(_)
        | DecoderErrorKind::GetProbeError(_)
        | DecoderErrorKind::NoSampleRate
        | DecoderErrorKind::NoChannels => StartAction::Skip,
        _ => StartAction::Pause,
    }
}
