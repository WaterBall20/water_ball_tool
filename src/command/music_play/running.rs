use crate::command::music_play::decoder::{
    Decoder, DecoderError, DecoderErrorKind, DecoderEventDirectSink, DecoderEventSink,
    DecoderEventType, DecoderResult,
};
use crate::command::music_play::effect::fade::FadeEffect;
use crate::command::music_play::play_list::{PlayList, Track};
use crate::command::music_play::player::{
    Player, PlayerEventChannelSink, PlayerEventSink, PlayerEventType,
};
use colored::Colorize;
use indicatif::{MultiProgress, ProgressStyle};
use std::any::Any;
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};
use std::{io, thread};
use tracing::{debug, error, info};

pub(super) struct Running {
    play_object: Arc<Mutex<PlayObject>>,
    thread_control: ThreadControl,
}

struct PlayObject {
    // 播放列表
    play_list: Arc<Mutex<PlayList>>,
    // 播放器
    player: Player,
    // 停止标志
    run_stop: bool,
    //这次歌曲
    this_track: Arc<Mutex<Option<Track>>>,
    //UI这次歌曲
    ui_track: Arc<Mutex<Option<Track>>>,
    // 上次进度
    last_play_pos: Duration,
    // 上次进度相等次数
    last_play_pos_eq_count: usize,
    // 上次进度相等起始时间
    last_pos_eq_start_time: Duration,
    next_content: NextContent,
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

impl Drop for Running {
    fn drop(&mut self) {
        _ = self.run_stop_inner();
    }
}

impl Running {
    ///创建运行时
    pub(super) fn create(play_list: PlayList, mp: &MultiProgress) -> Result<Self, Box<dyn Error>> {
        let mut player = Player::new()?;

        let this_track = Arc::new(Mutex::new(None));
        let ui_track = Arc::new(Mutex::new(None));
        //注册事件
        {
            let this_track = this_track.clone();
            let ui_track = ui_track.clone();
            player.add_event(Box::new(PlayerEventChannelSink::new(move |e| {
                debug!("播放事件触发: {e:?}");
                if let Ok(e) = e.result() {
                    match e {
                        PlayerEventType::TrackEnded => {
                            ui_track
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .clone_from(
                                    &this_track
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

        let play_object = Arc::new(Mutex::new(PlayObject {
            play_list: Arc::new(Mutex::new(play_list)),
            player,
            run_stop: false,
            this_track,
            ui_track,
            last_play_pos: Duration::default(),
            last_play_pos_eq_count: 0,
            last_pos_eq_start_time: Duration::default(),
            next_content: NextContent {
                next_id: Arc::new(AtomicUsize::new(0)),
                err_count: Arc::new(AtomicUsize::new(0)),
            },
        }));
        //控制变量
        let condvar = Arc::new(Condvar::new());
        //创建线程
        //UI线程
        let ui_thread = {
            let condvar = condvar.clone();
            let play_object = play_object.clone();
            Self::create_ui_thread(condvar, play_object, mp)
        }?;
        let thread_control = ThreadControl {
            ui_thread: Some(ui_thread),
            condvar,
        };
        Ok(Self {
            play_object,
            thread_control,
        })
    }

    fn create_ui_thread(
        condvar: Arc<Condvar>,
        play_object: Arc<Mutex<PlayObject>>,
        mp: &MultiProgress,
    ) -> io::Result<JoinHandle<()>> {
        let pb = crate::command::create_pb(mp);
        thread::Builder::new().name("ui".to_string()).spawn(move || {
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{prefix:<8.bold.green} [{wide_bar:.cyan/blue}] {percent:>3.bold.magenta}%  {msg}")
                        .expect("设置进度条模板失败")
                        .progress_chars("█▉▊▋▌▍▎▏ ")
                );
                pb.set_prefix("暂停中");
            loop {
                let mut play_object = play_object
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if play_object.run_stop {
                    break; // 如果正在释放就退出线程
                }
                let play_pos = play_object.player.get_pos();
                // 挂起检查
                if play_object.last_play_pos == play_pos {
                    //检查是否是最初
                    if play_object.last_play_pos_eq_count == 0 {
                        play_object.last_pos_eq_start_time = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default();
                    }
                    play_object.last_play_pos_eq_count += 1;
                    //次数检查和超时检查
                    if play_object.last_play_pos_eq_count > 3
                        && SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .checked_sub(play_object.last_pos_eq_start_time)
                            .unwrap_or_default()
                            > Duration::from_secs(10)
                    {
                        condvar.notify_all(); //唤醒所有线程
                    }
                } else {
                    //重置数量
                    play_object.last_play_pos_eq_count = 0;
                }
                play_object.last_play_pos = play_pos;
                let track_lock = play_object
                    .this_track
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(track) = &*track_lock {
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
                        )
                        .cyan(); // 时间: 青色

                        let volume_str =
                            format!("volume: {:.0}%", play_object.player.volume() * 100.0).yellow();
                        // 音量: 黄色

                        let title_str = track.title().bright_white().bold(); // 标题: 加粗亮白 (最醒目)
                        let artist_str = track.artist().bright_blue(); // 艺术家: 亮蓝
                        let path_str = track.path().display().to_string().dimmed(); // 路径: 暗淡弱化

                        // ---- 拼装为 msg ----
                        pb.set_message(format!(
                            "[{time_str}] [{volume_str}]\
                            \n{label_title}  : {title_str}\
                            \n{label_artist} : {artist_str}\
                            \n{label_path}   : {path_str}",
                            label_title = "Title".green(),
                            label_artist = "Artist".green(),
                            label_path = "Path".green(),
                        ));

                    drop(track_lock);
                    play_object = condvar
                        .wait_timeout(play_object, Duration::from_millis(50))
                        .expect("Condvar won't fail")
                        .0;
                } else {
                        pb.set_prefix("异常暂停中");
                    drop(track_lock);
                    play_object = condvar.wait(play_object).expect("Condvar won't fail");
                }
                //暂停时挂起
                if !play_object.player.is_playing() && !play_object.run_stop {
                    pb.set_prefix("暂停中");
                    play_object = condvar.wait(play_object).expect("Condvar won't fail");
                    if play_object.run_stop {
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
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .play()?;
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 暂停
    pub(super) fn pause(&mut self) -> Result<(), Box<dyn Error>> {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pause()?;
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 下一曲
    pub(super) fn next(&mut self) -> Result<(), Box<dyn Error>> {
        let mut play_object = self
            .play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = play_object.next()?;
        if let Some(track) = r {
            info!(
                "换曲为, Title: {}, Artist: {}, \
                \n Path: {}",
                track.title(),
                track.artist(),
                track.path().display()
            );
        }
        drop(play_object);
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 获取音量
    pub fn volume(&self) -> f32 {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .volume()
    }

    /// 设置音量
    pub(super) fn set_volume(&mut self, value: f32) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_volume(value);
    }

    /// 增加音量
    pub(super) fn add_volume(&mut self, value: f32) {
        let mut play_object = self
            .play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let this_volume = play_object.player.volume();
        let new_volume = this_volume + value;
        play_object.set_volume(new_volume.clamp(0.0, 1.0));
    }

    /// 增加进度
    pub(super) fn add_pos(&mut self, value: Duration) -> Result<(), Box<dyn Error>> {
        todo!();
        let play_object = self
            .play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let this_pos = play_object.player.get_pos();
        let new_pos = this_pos + value;
        play_object.player.try_seek(new_pos)
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
            let mut play_object = self
                .play_object
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            play_object.run_stop = true;
            _ = play_object.pause();
            play_object.player.try_stop().expect("停止播放发生错误");
        }
        self.thread_control.condvar.notify_all();
        self.join_thread()?;

        Ok(())
    }
}
impl PlayObject {
    ///播放
    fn play(&mut self) -> Result<(), Box<dyn Error>> {
        self.player.play()
    }

    ///暂停
    fn pause(&mut self) -> Result<(), Box<dyn Error>> {
        self.player.pause()
    }

    /// 获取音量
    fn volume(&self) -> f32 {
        self.player.volume()
    }

    /// 设置音量
    fn set_volume(&mut self, value: f32) {
        self.player.set_volume(value);
    }

    ///下一曲
    fn next(&mut self) -> DecoderResult<Option<Track>> {
        Self::next_inner(
            &self.play_list,
            false,
            self.player.clone(),
            &self.this_track,
            Some(&self.ui_track),
            &self.next_content,
        )
    }

    fn next_inner(
        play_list: &Arc<Mutex<PlayList>>,
        play_end: bool,
        player: Player,
        this_track: &Arc<Mutex<Option<Track>>>,
        ui_track: Option<&Arc<Mutex<Option<Track>>>>,
        next_content: &NextContent,
    ) -> DecoderResult<Option<Track>> {
        let mut player = player;
        let mut play_list_lock = play_list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next_id = &next_content.next_id;
        //临界区处理
        while player.get_decoder_eof() {
            if play_end {
                //自旋等待
                thread::sleep(Duration::from_millis(1));
            } else {
                //缓冲区部分清理
                player.clear_decoder_eof_buf();
                //立刻指定UI
                if let Some(ui_track) = &ui_track {
                    let this_track = this_track
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    *ui_track
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = this_track;
                }
                return Ok(this_track
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone());
            };
        }
        let track = play_list_lock.next(play_end, next_id.load(Ordering::Acquire));
        if let Some(t) = &track {
            info!("换曲： {:?}", t.track());
            next_id.store(t.next_id(), Ordering::Release);

            let mut decoder = loop {
                let decoder = Decoder::new(
                    player.ring_buffer().clone(),
                    t.track().path(),
                    player.get_sample_rate(),
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
                                    this_track,
                                    ui_track,
                                    next_content,
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

            let play_list = play_list.clone();
            {
                let this_track = this_track.clone();
                let player = player.clone();
                let next_content = next_content.clone();
                decoder.add_event(Box::new(DecoderEventDirectSink::new(move |e| {
                    debug!("解码器事件触发：{e:?}");
                    match e.result() {
                        Ok(decoder_event) => match decoder_event {
                            DecoderEventType::Eof => {
                                if let Err(e) = Self::next_inner(
                                    &play_list,
                                    true,
                                    player.clone(),
                                    &this_track,
                                    None,
                                    &next_content,
                                ) {
                                    error!("自动换曲发生错误，err:{e}");
                                }
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
                player.set_decoder(decoder)?;
            }
            this_track
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
