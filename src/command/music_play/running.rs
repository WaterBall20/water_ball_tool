use crate::command::music_play::decoder::{
    Decoder, DecoderEventChannelSink, DecoderEventSink, DecoderEventType, DecoderResult,
};
use crate::command::music_play::play_list::{PlayList, Track};
use crate::command::music_play::player::Player;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressStyle};
use std::any::Any;
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};
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
    // 播放标志
    is_playing: bool,
    // 停止标志
    run_stop: bool,
    // 换曲ID
    next_id: Arc<AtomicUsize>,
    //
    this_track: Arc<Mutex<Option<Track>>>,
    // 上次进度
    last_play_pos: Duration,
    // 上次进度相等次数
    last_play_pos_eq_count: usize,
    // 上次进度相等起始时间
    last_pos_eq_start_time: Duration,
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
    pub(super) fn create(
        play_list: PlayList,
        mp: Option<&MultiProgress>,
    ) -> Result<Self, Box<dyn Error>> {
        let player = Player::new()?;
        let play_object = Arc::new(Mutex::new(PlayObject {
            play_list: Arc::new(Mutex::new(play_list)),
            player,
            is_playing: false,
            run_stop: false,
            next_id: Arc::new(AtomicUsize::new(0)),
            this_track: Arc::new(Mutex::new(None)),
            last_play_pos: Duration::default(),
            last_play_pos_eq_count: 0,
            last_pos_eq_start_time: Duration::default(),
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
        mp: Option<&MultiProgress>,
    ) -> std::io::Result<JoinHandle<()>> {
        let pb = crate::command::create_pb(mp);
        thread::Builder::new().name("ui".to_string()).spawn(move || {
            let pb = pb;
            if let Some(pb) = &pb {
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{prefix:<8.bold.green} [{wide_bar:.cyan/blue}] {percent:>3.bold.magenta}%  {msg}")
                        .expect("设置进度条模板失败")
                        .progress_chars("█▉▊▋▌▍▎▏ ")
                );
                pb.set_prefix("暂停中");
            }
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
                    if let Some(pb) = &pb {
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
                    }
                    drop(track_lock);
                    play_object = condvar
                        .wait_timeout(play_object, Duration::from_millis(50))
                        .expect("Condvar won't fail")
                        .0;
                } else {
                    if let Some(pb) = &pb {
                        pb.set_prefix("异常暂停中");
                    }
                    drop(track_lock);
                    play_object = condvar.wait(play_object).expect("Condvar won't fail");
                }
                //暂停时挂起
                if !play_object.is_playing {
                    if let Some(pb) = &pb {
                        pb.set_prefix("暂停中");
                    }
                    play_object = condvar.wait(play_object).expect("Condvar won't fail");
                    if play_object.run_stop {
                        break; // 如果正在释放就退出线程
                    }
                    if let Some(pb) = &pb {
                        pb.set_prefix("播放中");
                    }
                }
            }
            debug!("[UI线程]线程正在退出");
        })
    }

    /// 播放
    pub(super) fn play(&mut self) -> Result<(), cpal::Error> {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .play()?;
        self.thread_control.condvar.notify_all();
        Ok(())
    }

    /// 暂停
    pub(super) fn pause(&mut self) -> Result<(), cpal::Error> {
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
    fn play(&mut self) -> Result<(), cpal::Error> {
        self.is_playing = true;
        self.player.play()
    }

    ///暂停
    fn pause(&mut self) -> Result<(), cpal::Error> {
        self.is_playing = false;
        self.player.pause()
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
            &self.next_id,
            self.player.clone(),
            self.this_track.clone(),
        )
    }

    fn next_inner(
        play_list: &Arc<Mutex<PlayList>>,
        play_end: bool,
        next_id: &Arc<AtomicUsize>,
        player: Player,
        this_track: Arc<Mutex<Option<Track>>>,
    ) -> DecoderResult<Option<Track>> {
        let mut player = player;
        let mut play_list_lock = play_list.lock().unwrap_or_else(|err| err.into_inner());
        let track = play_list_lock.next(play_end, next_id.load(Ordering::Acquire));
        if let Some(t) = &track {
            info!("换曲： {:?}", t.track());
            next_id.store(t.next_id(), Ordering::Release);
            let mut decoder =
                Decoder::new(player.ring_buffer().clone(), t.track().path().to_path_buf())?;

            let play_list = play_list.clone();
            let next_id = next_id.clone();
            {
                let this_track = this_track.clone();
                let audio = player.clone();
                decoder.add_event(Box::new(DecoderEventChannelSink::new(move |e| {
                    debug!("解码器事件触发：{e:?}");
                    match e.result() {
                        Ok(decoder_event) => match decoder_event {
                            DecoderEventType::Eof => {
                                if let Err(e) = Self::next_inner(
                                    &play_list,
                                    true,
                                    &next_id,
                                    audio.clone(),
                                    this_track.clone(),
                                ) {
                                    error!("自动换曲发生错误： {e}");
                                }
                            }
                        },
                        Err(err) => {
                            error!("发生解码错误事件： {err:?}");
                        }
                    }
                })));
            }
            player.set_decoder(decoder)?;
            this_track
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .replace(t.track().clone());
            Ok(Some(t.track().clone()))
        } else {
            if play_end {
                player.pause().unwrap_or_else(|e| {
                    error!("暂停失败: err: {e}");
                });
                info!("没有下一曲，将暂停");
            } else {
                error!("切换下一曲发生错误, 不存在");
            }
            Ok(None)
        }
    }
}
