use crate::command::music_play::LoopMode;
use crate::command::music_play::play_list::{NextTrack, PlayList};
use colored::Colorize;
use indicatif::{MultiProgress, ProgressStyle};
use rodio::source::SeekError;
use rodio::{Decoder, MixerDeviceSink, Player};
use std::any::Any;
use std::error::Error;
use std::fs::File;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};
use tracing::{debug, error, info, warn};

pub(super) struct Running {
    play_object: Arc<Mutex<PlayObject>>,
    thread_control: ThreadControl,
}

struct PlayObject {
    // 播放列表
    play_list: PlayList,
    handle: MixerDeviceSink,
    player: Player,
    // 播放标志
    is_playing: bool,
    // 停止标志
    run_stop: bool,
    // 换曲ID
    next_id: usize,
    // 上次进度
    last_play_pos: Duration,
    // 上次进度相等次数
    last_play_pos_eq_count: usize,
    // 上次进度相等起始时间
    last_pos_eq_start_time: Duration,
}

struct ThreadControl {
    next_thread: Option<JoinHandle<()>>,
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
        let handle = rodio::DeviceSinkBuilder::open_default_sink()?;
        let player = Player::connect_new(handle.mixer());
        player.pause();
        let play_object = Arc::new(Mutex::new(PlayObject {
            play_list,
            handle,
            player,
            is_playing: false,
            run_stop: false,
            next_id: 0,
            last_play_pos: Duration::default(),
            last_play_pos_eq_count: 0,
            last_pos_eq_start_time: Duration::default(),
        }));
        //控制变量
        let condvar = Arc::new(Condvar::new());
        //创建线程
        //换曲线程
        let next_thread = {
            let condvar = condvar.clone();
            let play_object = play_object.clone();
            Self::create_next_thread(condvar, play_object)
        };
        //UI线程
        let ui_thread = {
            let condvar = condvar.clone();
            let play_object = play_object.clone();
            Self::create_ui_thread(condvar, play_object, mp)
        };
        let next_thread = ThreadControl {
            next_thread: Some(next_thread),
            ui_thread: Some(ui_thread),
            condvar,
        };
        Ok(Self {
            play_object,
            thread_control: next_thread,
        })
    }

    fn create_ui_thread(
        condvar: Arc<Condvar>,
        play_object: Arc<Mutex<PlayObject>>,
        mp: Option<&MultiProgress>,
    ) -> JoinHandle<()> {
        let pb = crate::command::create_pb(mp);
        thread::spawn(move || {
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
                if let Some(track) = play_object.play_list.get_current_track() {
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
                            format!("volume: {:.0}%", play_object.player.volume() * 100.0).yellow(); // 音量: 黄色

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
                    play_object = condvar
                        .wait_timeout(play_object, Duration::from_millis(50))
                        .expect("Condvar won't fail")
                        .0;
                } else {
                    if let Some(pb) = &pb {
                        pb.set_prefix("异常暂停中");
                    }
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

    /// 创建换曲线程
    fn create_next_thread(
        condvar: Arc<Condvar>,
        play_object: Arc<Mutex<PlayObject>>,
    ) -> JoinHandle<()> {
        fn next_inner(play_object: &mut PlayObject, next_id: usize) {
            //执行换曲
            let r = play_object.next_inner(true, next_id);
            match r {
                Ok(o) => {
                    if let Some(o) = o {
                        let track = o.track();
                        info!(
                            "自动换曲为, Title: {}, Artist: {}, \
                            \n Path: {}",
                            track.title().bright_white().bold(),
                            track.artist().bright_blue(),
                            track.path().display()
                        );
                    } else if let LoopMode::Sequential = play_object.play_list.loop_mod() {
                        info!("自动换曲，顺序播放已结束，暂停播放");
                        play_object.pause();
                    }
                }
                Err(e) => {
                    error!("自动换曲发生错误 {e:?}");
                }
            }
        }
        thread::Builder::new()
            .name("next_thread".to_string())
            .spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(10));
                    let mut play_object = play_object
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if !play_object.is_playing {
                        play_object = condvar.wait(play_object).expect("Condvar won't fail");
                        if play_object.run_stop {
                            break; // 如果正在释放就退出线程
                        }
                        continue;
                    }
                    // 如果运行时退出就退出线程
                    if play_object.run_stop {
                        break;
                    }
                    //如果正在播放
                    let play_pos = play_object.player.get_pos();
                    // 挂起检查、次数检查和超时检查
                    if play_object.last_play_pos == play_pos
                        && play_object.last_play_pos_eq_count > 3
                    {
                        let last_eq_dt = SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .checked_sub(play_object.last_pos_eq_start_time)
                            .unwrap_or_default();
                        if last_eq_dt > Duration::from_secs(10) {
                            warn!("检测到播放挂起，将换曲，超时: {last_eq_dt:?}",);
                            let next_id = play_object.next_id;
                            next_inner(&mut play_object, next_id);
                            //复位
                            play_object.last_play_pos_eq_count = 0;
                            continue;
                        }
                    }
                    if let Some(track) = play_object.play_list.get_current_track() {
                        let track_dur = *track.duration();
                        let next_id = play_object.next_id;
                        let wait_dur = track_dur
                            .checked_sub(play_pos)
                            .unwrap_or(Duration::default());
                        // 判断是否需要挂起
                        if wait_dur.as_millis() > 0 {
                            let (guard, timeout_result) = condvar
                                .wait_timeout(play_object, wait_dur)
                                .expect("Condvar won't fail");
                            play_object = guard;

                            // 如果运行时退出就退出线程
                            if play_object.run_stop {
                                break;
                            }
                            if timeout_result.timed_out() {
                                // 检查是否播放和时间
                                if play_object.is_playing
                                    && track_dur
                                    .checked_sub(play_pos)
                                    .unwrap_or(Duration::default())
                                    .as_millis()
                                    <= 10
                                {
                                    //执行换曲
                                    next_inner(&mut play_object, next_id);
                                }
                            }
                        } else {
                            //执行换曲
                            next_inner(&mut play_object, next_id);
                        }
                    } else {
                        warn!("[换曲线程]无法获取当前播放音轨，线程挂起");
                        play_object = condvar.wait(play_object).expect("Condvar won't fail");
                    }
                }
            })
            .expect("无法创建线程")
    }

    /// 播放
    pub(super) fn play(&mut self) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .play();
        self.thread_control.condvar.notify_all();
    }

    /// 暂停
    pub(super) fn pause(&mut self) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pause();
        self.thread_control.condvar.notify_all();
    }

    /// 下一曲
    pub(super) fn next(&mut self) -> Result<(), Box<dyn Error>> {
        let mut play_object = self
            .play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = play_object.next()?;
        if let Some(t) = r {
            let track = t.track();
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
    pub(super) fn set_volume(&mut self, value: f64) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_volume(value);
    }

    /// 增加音量
    pub(super) fn add_volume(&mut self, value: f64) {
        let mut play_object = self
            .play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let this_volume = play_object.player.volume();
        let new_volume = this_volume + value;
        play_object.set_volume(new_volume.clamp(0.0, 1.0));
    }

    /// 增加进度
    pub(super) fn add_pos(&mut self, value: Duration) -> Result<(), SeekError> {
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
        if let Some(h) = self.thread_control.next_thread.take() {
            h.join()?;
        }
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
            play_object.pause();
            play_object.player.clear();
            play_object.run_stop = true;
        }
        self.thread_control.condvar.notify_all();
        self.join_thread()?;

        Ok(())
    }
}
impl PlayObject {
    ///播放
    fn play(&mut self) {
        self.is_playing = true;
        self.player.play();
    }

    ///暂停
    fn pause(&mut self) {
        self.is_playing = false;
        self.player.pause();
    }

    /// 设置音量
    fn set_volume(&mut self, value: f64) {
        self.player.set_volume(value);
    }

    ///下一曲
    fn next(&mut self) -> Result<Option<NextTrack<'_>>, Box<dyn Error>> {
        self.next_inner(false, self.next_id)
    }

    //下一曲内部实现
    fn next_inner(
        &mut self,
        play_end: bool,
        next_id: usize,
    ) -> Result<Option<NextTrack<'_>>, Box<dyn Error>> {
        let paused = self.player.is_paused();
        let loop_mode = self.play_list.loop_mod();
        let next_track = self.play_list.next(play_end, next_id);
        if let Some(next_track) = &next_track {
            let track = next_track.track();
            let file = File::open(track.path())
                .map_err(|e| io::Error::new(e.kind(), format!("打开文件错误, err: {e}")))?;
            self.player.clear();
            if let LoopMode::RepeatOne = loop_mode {
                let d = Decoder::new_looped(file)?;
                self.player.append(d);
            } else {
                let d = Decoder::new(file)?;
                self.player.append(d);
            }
            self.next_id = next_track.next_id();
            if !paused {
                self.player.play();
            }
        }
        Ok(next_track)
    }
}
