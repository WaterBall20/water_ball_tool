use crate::command::music_play::decoder::{
    Decoder, DecoderError, DecoderErrorKind, DecoderEventDirectSink, DecoderEventSink,
    DecoderEventType, DecoderResult,
};
use crate::command::music_play::effect::fade::FadeEffect;
use crate::command::music_play::effect::runtime::PreEffectRuntime;
use crate::command::music_play::play_list::{PlayList, Track};
use crate::command::music_play::player::{
    Player, PlayerEventChannelSink, PlayerEventSink, PlayerEventType,
};
use crate::command::music_play::tools::{DurationStats, DurationStatsWindow, TimeMetrics};
use colored::Colorize;
use indicatif::{MultiProgress, ProgressStyle};
use std::any::Any;
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};
use std::{io, thread};
use tracing::{debug, error, info};

pub(super) struct Runtime {
    play_object: Arc<PlayObject>,
    thread_control: ThreadControl,
    ui_content: Arc<Mutex<UIContent>>,
    // 停止标志
    run_stop: Arc<AtomicBool>,
}

struct PlayObject {
    // 播放列表
    play_list: Arc<Mutex<PlayList>>,
    // 播放器
    player: Player,
    //解码中的歌曲
    decoder_track: Arc<Mutex<Option<Track>>>,
    next_content: NextContent,
    time_metrics: Arc<TimeMetrics>,
}

#[derive(Clone)]
struct NextContent {
    // 换曲ID
    next_id: Arc<AtomicUsize>,
    //连续发生错误次数
    err_count: Arc<AtomicUsize>,
}

struct ThreadControl {
    ui_thread: Option<JoinHandle<()>>,
    condvar: Arc<Condvar>,
}

struct UIContent {
    //UI这次歌曲
    ui_track: Option<Track>,
    // 上次进度
    last_play_pos: Duration,
    // 上次进度相等次数
    last_play_pos_eq_count: usize,
    // 上次进度相等起始时间
    last_pos_eq_start_time: Duration,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        _ = self.run_stop_inner();
    }
}

impl Runtime {
    ///创建运行时
    pub(super) fn create(
        play_list: PlayList,
        mp: &MultiProgress,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let mut player = Player::new()?;
        let decoder_track = Arc::new(Mutex::new(None));

        let play_object = Arc::new(PlayObject {
            play_list: Arc::new(Mutex::new(play_list)),
            player: player.clone(),
            decoder_track: decoder_track.clone(),
            next_content: NextContent {
                next_id: Arc::new(AtomicUsize::new(0)),
                err_count: Arc::new(AtomicUsize::new(0)),
            },
            time_metrics: player.time_metrics().clone(),
        });
        //控制变量
        let condvar = Arc::new(Condvar::new());
        let run_stop = Arc::new(AtomicBool::new(false));
        let ui_content = Arc::new(Mutex::new(UIContent {
            ui_track: None,
            last_play_pos: Duration::from_millis(0),
            last_play_pos_eq_count: 0,
            last_pos_eq_start_time: Duration::from_millis(0),
        }));
        //创建线程
        //UI线程
        let ui_thread = {
            let condvar = condvar.clone();
            let play_object = play_object.clone();
            Self::create_ui_thread(
                condvar,
                play_object,
                mp,
                run_stop.clone(),
                ui_content.clone(),
            )
        }?;
        let thread_control = ThreadControl {
            ui_thread: Some(ui_thread),
            condvar,
        };

        //注册事件
        {
            let decoder_track = decoder_track.clone();
            let ui_content = ui_content.clone();
            player.add_event(Box::new(PlayerEventChannelSink::new(move |e| {
                debug!("播放事件触发: {e:?}");
                if let Ok(e) = e.result() {
                    match e {
                        PlayerEventType::TrackEnded => {
                            ui_content
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .ui_track
                                .clone_from(
                                    &decoder_track
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                                );
                        }
                    }
                }
            })));
        }
        //添加效果(淡出淡入)
        player.add_audio_post_effect(FadeEffect::new());

        Ok(Self {
            play_object,
            thread_control,
            ui_content,
            run_stop,
        })
    }

    fn create_ui_thread(
        condvar: Arc<Condvar>,
        play_object: Arc<PlayObject>,
        mp: &MultiProgress,
        run_stop: Arc<AtomicBool>,
        ui_content: Arc<Mutex<UIContent>>,
    ) -> io::Result<JoinHandle<()>> {
        fn time_take(
            duration_stats: &DurationStats,
            duration_stats_window: &mut DurationStatsWindow,
        ) {
            if let Some(d) = duration_stats.take() {
                *duration_stats_window = d;
            }
        }
        fn fmt_ms(ns: u64) -> String {
            let ms = ns as f64 / 1_000_000.0;
            format!("{ms:09.4}ms")
        }
        let pb = crate::command::create_pb(mp);

        thread::Builder::new().name("ui".to_string()).spawn(move || {
            //时间统计
            let mut time_decoder = DurationStatsWindow::default();
            let mut time_decoder_write_wait = DurationStatsWindow::default();
            let mut time_effect_read_wait = DurationStatsWindow::default();
            let mut time_effect_process = DurationStatsWindow::default();
            let mut time_effect_write_wait = DurationStatsWindow::default();
            let mut time_callback_copy = DurationStatsWindow::default();
            let mut time_callback = DurationStatsWindow::default();
            let mut time_callback_post_effect = DurationStatsWindow::default();
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{prefix:<8.bold.green} [{wide_bar:.cyan/blue}] {percent:>3.bold.magenta}%  {msg}")
                        .expect("设置进度条模板失败")
                        .progress_chars("█▉▊▋▌▍▎▏ ")
                );
                pb.set_prefix("暂停中");
            loop {
                let mut ui_content = ui_content.lock().unwrap_or_else
                (std::sync::PoisonError::into_inner);
                if run_stop.load(Ordering::Acquire) {
                    break; // 如果正在释放就退出线程
                }
                let play_pos = play_object.player.get_pos();
                // 挂起检查
                if ui_content.last_play_pos == play_pos {
                    //检查是否是最初
                    if ui_content.last_play_pos_eq_count == 0 {
                        ui_content.last_pos_eq_start_time = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default();
                    }
                    ui_content.last_play_pos_eq_count += 1;
                    //次数检查和超时检查
                    if ui_content.last_play_pos_eq_count > 3
                        && SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                        .checked_sub(ui_content.last_pos_eq_start_time)
                            .unwrap_or_default()
                            > Duration::from_secs(10)
                    {
                        condvar.notify_all(); //唤醒所有线程
                    }
                } else {
                    //重置数量
                    ui_content.last_play_pos_eq_count = 0;
                }
                ui_content.last_play_pos = play_pos;
                if let Some(track) = &ui_content.ui_track {
                    let track_dur = *track.duration();

                    let play_pos_millis = play_pos.as_millis();
                    let play_pos_secs = play_pos_millis / 1000;
                    let play_pos_minutes = play_pos_secs / 60;
                    let track_dur_millis = track_dur.as_millis();
                    let track_dur_secs = track_dur_millis / 1000;
                    let track_dur_minutes = track_dur_secs / 60;
                    pb.set_length(u64::try_from(track_dur_millis).unwrap_or(u64::MAX));
                    pb.set_position(u64::try_from(play_pos_millis).unwrap_or(u64::MAX));

                    // ---- 各字段分别着色 / Per-field coloring ----
                    let time_str = format!(
                        "{:>2}:{:0>2} / {:>2}:{:0>2}",
                        play_pos_minutes,
                        play_pos_secs % 60,
                        track_dur_minutes,
                        track_dur_secs % 60,
                    ).cyan(); // 时间: 青色

                    let volume_str =
                            format!("volume: {:.0}%", play_object.player.volume() * 100.0).yellow();
                    // 音量: 黄色

                    let title_str = track.title().bright_white().bold(); // 标题: 加粗亮白 (最醒目)
                    let artist_str = track.artist().bright_blue(); // 艺术家: 亮蓝
                    let path_str = track.path().display().to_string().dimmed(); // 路径: 暗淡弱化

                    //时间统计
                    let time_metrics_str = {
                        let time_m = &play_object.time_metrics;
                        time_take(&time_m.decode, &mut time_decoder);
                        time_take(&time_m.decode_write_wait, &mut time_decoder_write_wait);
                        time_take(&time_m.effect_read_wait, &mut time_effect_read_wait);
                        time_take(&time_m.effect_process, &mut time_effect_process);
                        time_take(&time_m.effect_write_wait, &mut time_effect_write_wait);
                        time_take(&time_m.callback_total, &mut time_callback);
                        time_take(&time_m.callback_copy, &mut time_callback_copy);
                        time_take(&time_m.callback_post_effect, &mut time_callback_post_effect);
                        format!("{title: <6}:\
                        \ndecoder: {decoder}, write_wait: {decoder_write_wait}\
                        \neffect: read_wait: {effect_read_wait}, process: {effect_process}, \
                        write_wait: {effect_write_wait}\
                        \ncallback: {callback}, copy: {callback_copy}, post_effect:\
                        {callback_post_effect}",
                                title = "Time".green(),
                                decoder = fmt_ms(time_decoder.max_ns).green(),
                                decoder_write_wait = fmt_ms(time_decoder_write_wait.max_ns).blue(),
                                effect_read_wait = fmt_ms(time_effect_read_wait.max_ns).yellow(),
                                effect_process = fmt_ms(time_effect_process.max_ns).red(),
                                effect_write_wait = fmt_ms(time_effect_write_wait.max_ns).blue(),
                                callback = fmt_ms(time_callback.max_ns).green(),
                                callback_copy = fmt_ms(time_callback_copy.max_ns).yellow(),
                                callback_post_effect = fmt_ms(time_callback_post_effect.max_ns).red()
                        )
                    };

                    // ---- 拼装为 msg ----
                    pb.set_message(format!(
                        "[{time_str}] [{volume_str}]\
                        \n{label_title: <6}: {title_str}\
                        \n{label_artist: <6}: {artist_str}\
                        \n{label_path: <6}: {path_str}\
                        \n{time_metrics_str}",
                        label_title = "Title".green(),
                        label_artist = "Artist".green(),
                        label_path = "Path".green(),
                    ));


                    ui_content = condvar
                        .wait_timeout(ui_content, Duration::from_millis(50))
                        .expect("Condvar won't fail")
                        .0;
                } else {
                        pb.set_prefix("异常暂停中");
                    ui_content = condvar.wait(ui_content).expect("Condvar won't fail");
                    continue;
                }
                //暂停时挂起
                if !play_object.player.is_playing() && !run_stop.load(Ordering::Acquire) {
                    pb.set_prefix("暂停中");
                    ui_content = condvar.wait(ui_content).expect("Condvar won't fail");
                    if run_stop.load(Ordering::Acquire) {
                        break; // 如果正在释放就退出线程
                    }
                    pb.set_prefix("播放中");

                }
            }
            debug!("[UI线程]线程正在退出");
        })
    }

    /// 播放
    pub(super) fn play(&mut self) -> Result<(), Box<dyn Error>> {
        self.play_object.play()?;
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 暂停
    pub(super) fn pause(&mut self) -> Result<(), Box<dyn Error>> {
        self.play_object.pause()?;
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 下一曲
    pub(super) fn next(&mut self) -> Result<(), Box<dyn Error>> {
        let mut ui_content = self
            .ui_content
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = self
            .play_object
            .next(&mut ui_content.ui_track, &self.thread_control.condvar)?;
        if let Some(track) = r {
            info!(
                "换曲为, Title: {}, Artist: {}, \
                \n Path: {}",
                track.title(),
                track.artist(),
                track.path().display()
            );
        }
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 获取音量
    pub fn volume(&self) -> f32 {
        self.play_object.volume()
    }

    /// 设置音量
    pub(super) fn set_volume(&self, value: f32) {
        self.play_object.set_volume(value);
    }

    /// 增加音量
    pub(super) fn add_volume(&self, value: f32) {
        let this_volume = self.play_object.player.volume();
        let new_volume = this_volume + value;
        self.play_object.set_volume(new_volume.clamp(0.0, 1.0));
    }

    /// 增加进度
    pub(super) fn add_pos(&mut self, value: Duration) -> Result<(), Box<dyn Error>> {
        todo!();
    }

    /// 等待线程
    pub(super) fn join_thread(&mut self) -> Result<(), Box<dyn Any + Send + 'static>> {
        if let Some(h) = self.thread_control.ui_thread.take() {
            h.join()?;
        }
        Ok(())
    }

    /// 运行时停止
    pub(super) fn run_stop(self) -> Result<(), Box<dyn Any + Send + 'static>> {
        let mut run = self;
        run.run_stop_inner()
    }

    fn run_stop_inner(&mut self) -> Result<(), Box<dyn Any + Send + 'static>> {
        {
            self.run_stop.store(true, Ordering::Release);
            _ = self.play_object.pause();
            self.play_object
                .player
                .try_stop()
                .expect("停止播放发生错误");
        }
        self.thread_control.condvar.notify_all();
        self.join_thread()?;

        Ok(())
    }
}
impl PlayObject {
    ///播放
    fn play(&self) -> Result<(), Box<dyn Error>> {
        self.player.play()
    }

    ///暂停
    fn pause(&self) -> Result<(), Box<dyn Error>> {
        self.player.pause()
    }

    /// 获取音量
    fn volume(&self) -> f32 {
        self.player.volume()
    }

    /// 设置音量
    fn set_volume(&self, value: f32) {
        self.player.set_volume(value);
    }

    ///下一曲
    fn next(
        &self,
        ui_track: &mut Option<Track>,
        condvar: &Arc<Condvar>,
    ) -> DecoderResult<Option<Track>> {
        Self::next_inner(
            &self.play_list,
            false,
            self.player.clone(),
            &self.decoder_track,
            Some(ui_track),
            &self.next_content,
            self.player.pre_effect_runtime(),
            condvar,
        )
    }

    fn next_inner(
        play_list: &Arc<Mutex<PlayList>>,
        play_end: bool,
        player: Player,
        decoder_track: &Arc<Mutex<Option<Track>>>,
        ui_track: Option<&mut Option<Track>>,
        next_content: &NextContent,
        pre_effect_runtime: &PreEffectRuntime,
        condvar: &Arc<Condvar>,
    ) -> DecoderResult<Option<Track>> {
        let mut player = player;
        let mut play_list_lock = play_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next_id = &next_content.next_id;
        //临界区处理
        while player.get_pre_effect_item_eof() {
            if play_end {
                //自旋等待
                thread::sleep(Duration::from_millis(1));
            } else {
                //缓冲区
                player.clear_pre_effect_item_eof_buf();
                //立刻指定UI
                if let Some(ui_track) = ui_track {
                    let decoder_track = decoder_track
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    *ui_track = decoder_track;
                }
                return Ok(decoder_track
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone());
            }
        }
        let track = play_list_lock.next(play_end, next_id.load(Ordering::Acquire));
        if let Some(t) = &track {
            info!("换曲： {:?}", t.track());
            next_id.store(t.next_id(), Ordering::Release);

            let mut decoder = loop {
                let decoder = Decoder::new(
                    t.track().path(),
                    player.pre_effect_ring_buffer().clone(),
                    player.pre_effect_runtime().clone(),
                    player.time_metrics().clone(),
                );
                match decoder {
                    Ok(decoder) => break decoder,
                    Err(e) => {
                        let err_action = Self::new_decoder_err_action_inner(&e, next_content);
                        match err_action {
                            DecoderErrorAction::Skip => {
                                drop(play_list_lock); //必须提前释放，否则死锁
                                return Self::next_inner(
                                    play_list,
                                    play_end,
                                    player.clone(),
                                    decoder_track,
                                    ui_track,
                                    next_content,
                                    pre_effect_runtime,
                                    condvar,
                                );
                            }
                            DecoderErrorAction::AutoRetry => {}
                            DecoderErrorAction::Pause => {
                                if let Err(e) = player.pause() {
                                    error!("暂停发生错误, err: {e}");
                                }
                                return Err(e);
                            }
                        }
                    }
                }
            };

            {
                let condvar = condvar.clone();
                let play_list = play_list.clone();
                let track = t.track().clone();
                let decoder_track = decoder_track.clone();
                let player = player.clone();
                let next_content = next_content.clone();
                let pre_effect_runtime = pre_effect_runtime.clone();
                decoder.add_event(Box::new(DecoderEventDirectSink::new(move |e| {
                    debug!("解码器事件触发：{e:?}");
                    match e.result() {
                        Ok(decoder_event) => match decoder_event {
                            DecoderEventType::Eof => {
                                if let Err(e) = Self::next_inner(
                                    &play_list,
                                    true,
                                    player.clone(),
                                    &decoder_track,
                                    None,
                                    &next_content,
                                    &pre_effect_runtime,
                                    &condvar,
                                ) {
                                    error!("自动换曲发生错误，err:{e}");
                                }
                            }
                            DecoderEventType::Stop => {
                                debug!("解码退出事件, track: {track:?}");
                            }
                        },
                        Err(err) => {
                            if let Err(e) = player.pause() {
                                error!("暂停发生错误, err: {e}");
                            }
                            error!("发生解码错误事件, 将暂停： {err:?}");
                        }
                    }
                })));
            }
            if play_end {
                player.set_decoder_no_clear(decoder)?;
            } else {
                if let Some(ui_track) = ui_track {
                    ui_track.replace(t.track().clone());
                    condvar.notify_all();
                }
                player.set_decoder(decoder)?;
            }
            decoder_track
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace(t.track().clone());
            next_content.err_count.store(0, Ordering::Release);
            Ok(Some(t.track().clone()))
        } else {
            if play_end {
                if let Err(e) = player.pause() {
                    error!("暂停发生错误, err: {e}");
                }
                info!("没有下一曲，将暂停");
            } else {
                if let Err(e) = player.pause() {
                    error!("暂停发生错误, err: {e}");
                }
                error!("切换下一曲发生错误，不存在，将暂停");
            }
            pre_effect_runtime.send_no_next();
            next_content.err_count.store(0, Ordering::Release);

            Ok(None)
        }
    }

    //
    fn new_decoder_err_action_inner(
        e: &DecoderError,
        next_content: &NextContent,
    ) -> DecoderErrorAction {
        //增加错误计数
        next_content.err_count.fetch_add(1, Ordering::Release);
        if next_content.err_count.load(Ordering::Acquire) > 5 {
            error!("新建编码器发生多次失败（大于5次），将暂停");
            return DecoderErrorAction::Pause;
        }
        match e.kind() {
            DecoderErrorKind::IOError(io_e) => {
                match io_e.kind() {
                    // 永久性，建议跳过
                    io::ErrorKind::NotFound
                    | io::ErrorKind::InvalidData
                    | io::ErrorKind::Unsupported
                    | io::ErrorKind::IsADirectory
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::InvalidInput => {
                        error!("创建解码器发生了IO错误，将跳过，err: {e}");
                        DecoderErrorAction::Skip
                    }

                    // 可重试，暂停并给重试入口
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
                    | io::ErrorKind::PermissionDenied => {
                        error!("创建解码器发生了IO错误，尝试重试，err: {e}");
                        DecoderErrorAction::AutoRetry
                    }

                    // 未知或无法判断，保守暂停
                    _ => {
                        error!("创建解码器发生了无法自动处理的IO错误，将暂停，err: {e}");
                        DecoderErrorAction::Pause
                    }
                }
            }
            DecoderErrorKind::NoAudioTrack
            | DecoderErrorKind::NoAudioCodecParams
            | DecoderErrorKind::MakeAudioDecoderError(_)
            | DecoderErrorKind::GetProbeError(_)
            | DecoderErrorKind::NoSampleRate
            | DecoderErrorKind::NoChannels
            | DecoderErrorKind::ResamplerConstructionError(_) => {
                error!("创建解码器错误， 将跳过， err: {e}");
                DecoderErrorAction::Skip
            }
            _ => {
                error!("创建解码器发生了无法自动处理的错误，将暂停, err: {e}");
                DecoderErrorAction::Pause
            }
        }
    }
}

enum DecoderErrorAction {
    Skip,
    AutoRetry,
    Pause,
}
