use crate::command::music_play::LoopMode;
use crate::command::music_play::data::{NextTrack, PlayList};
use rodio::{Decoder, MixerDeviceSink, Player};
use std::any::Any;
use std::error::Error;
use std::fs::File;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub(super) struct Running {
    play_object: Arc<Mutex<PlayObject>>,
    next_thread: NextThread,
}

struct PlayObject {
    play_list: PlayList,
    handle: MixerDeviceSink,
    player: Player,
    run_stop: bool,
    next_id: u64,
}

struct NextThread {
    thread: Option<JoinHandle<()>>,
    condvar: Arc<Condvar>,
}

impl Drop for Running {
    fn drop(&mut self) {
        _ = self.run_stop_inner();
    }
}

impl Running {
    ///创建运行时
    pub(super) fn create(play_list: PlayList) -> Result<Self, Box<dyn Error>> {
        let handle = rodio::DeviceSinkBuilder::open_default_sink()?;
        let player = Player::connect_new(handle.mixer());
        player.pause();
        let play_object = Arc::new(Mutex::new(PlayObject {
            play_list,
            handle,
            player,
            run_stop: false,
            next_id: 0,
        }));
        //控制变量
        let condvar = Arc::new(Condvar::new());
        //创建线程
        let next_thread = {
            let condvar = condvar.clone();
            let play_object = play_object.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(10));
                    let mut play_object = play_object
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if play_object.player.is_paused() {
                        debug!("播放暂停，下一曲线程挂起");
                        play_object = condvar.wait(play_object).expect("Condvar won't fail");
                        debug!("下一曲线程唤醒");
                        if play_object.run_stop {
                            break; // 如果正在释放就退出线程
                        }
                        continue;
                    }
                    //如果正在播放
                    let play_pos = play_object.player.get_pos();
                    if let Some(track) = play_object.play_list.get_current_track() {
                        let track_dur = *track.duration();
                        let next_id = play_object.next_id;
                        // 判断是否需要挂起
                        if track_dur > play_pos {
                            let wait_dur = track_dur.checked_sub(play_pos).unwrap();
                            debug!("下一曲线程指定超时挂起，超时： {wait_dur:?}");
                            let (guard, timeout_result) = condvar
                                .wait_timeout(play_object, wait_dur)
                                .expect("Condvar won't fail");
                            play_object = guard;
                            // 如果运行时退出就退出线程
                            if play_object.run_stop {
                                break;
                            }
                            if timeout_result.timed_out() {
                                debug!("下一曲线程唤醒，已超时");
                                // 检查是否暂停
                                if !play_object.player.is_paused() {
                                    //执行换曲
                                    let r = play_object.next_inner(true, next_id);
                                    match r {
                                        Ok(o) => {
                                            if let Some(o) = o {
                                                let track = o.track();
                                                info!("自动换曲为, debug: {track:?}");
                                            } else {
                                                debug!("自动换曲，没有换曲");
                                                if let LoopMode::Sequential =
                                                    play_object.play_list.loop_mod()
                                                {
                                                    info!("自动换曲，顺序播放已结束，暂停播放");
                                                    play_object.player.pause();
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!("自动换曲发生错误 {e:?}");
                                        }
                                    }
                                }
                            } else {
                                debug!("下一曲线程唤醒，非超时");
                            }
                        }
                    } else {
                        info!("无法获取当前播放音轨，下一曲线程挂起");
                        play_object = condvar.wait(play_object).expect("Condvar won't fail");
                        info!("下一曲线程唤醒");
                    }
                }
                warn!("下一曲线程已退出");
            })
        };
        let next_thread = NextThread {
            thread: Some(next_thread),
            condvar,
        };
        Ok(Self {
            play_object,
            next_thread,
        })
    }

    /// 播放
    pub(super) fn play(&mut self) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .play();
        self.next_thread.condvar.notify_all();
    }

    /// 暂停
    pub(super) fn pause(&mut self) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pause();
        self.next_thread.condvar.notify_all();
    }

    /// 下一曲
    pub(super) fn next(&mut self) -> Result<(), Box<dyn Error>> {
        let mut play_object =  self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = play_object
            .next()?;
        if let Some(t) = r {
            let track = t.track();
            info!("换曲为, debug: {track:?}");
        }
        drop(play_object);
        self.next_thread.condvar.notify_all();
        Ok(())
    }

    /// 下一曲
    pub(super) fn set_volume(&mut self, value: f64) {
        self.play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_volume(value);
    }
    /// 等待线程
    pub(super) fn join_next_thread(&mut self) -> Result<(), Box<dyn Any + Send + 'static>> {
        if let Some(h) = self.next_thread.thread.take() {
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
        let mut play_object = self
            .play_object
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        play_object.player.clear();
        play_object.run_stop = true;
        self.next_thread.condvar.notify_all();
        if let Some(h) = self.next_thread.thread.take() {
            h.join()?;
        }
        Ok(())
    }
}
impl PlayObject {
    ///播放
    fn play(&mut self) {
        self.player.play();
    }

    ///暂停
    fn pause(&mut self) {
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
        next_id: u64,
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
