use crate::command::music_play::effect::{
    AudioFormat, Decision, PlayCmd, PostEffect, TransportAware,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// LUT 尺寸：下界 = sample_rate，另设 MIN_LUT_SIZE 兜底极低采样率
// ---------------------------------------------------------------------------
const MIN_LUT_SIZE: usize = 1024;

fn lut_size_for(sample_rate: u32) -> usize {
    (sample_rate as usize).max(MIN_LUT_SIZE)
}

// ---------------------------------------------------------------------------
// 全局 LUT 缓存：键 = sample_rate，值为长度为 size+1 的 f32 表
// ---------------------------------------------------------------------------
fn lut_cache() -> &'static Mutex<HashMap<u32, Arc<[f32]>>> {
    static CACHE: OnceLock<Mutex<HashMap<u32, Arc<[f32]>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_lut(sample_rate: u32) -> Arc<[f32]> {
    let mut map = lut_cache().lock().expect("LUT cache poisoned");
    if let Some(l) = map.get(&sample_rate) {
        return l.clone();
    }
    let size = lut_size_for(sample_rate);
    let mut v: Vec<f32> = Vec::with_capacity(size + 1);
    for i in 0..=size {
        let t = i as f64 / size as f64;
        // 半正弦包络：0.5 * (1 - cos(pi * t))
        v.push((0.5 * (1.0 - (std::f64::consts::PI * t).cos())) as f32);
    }
    let arc: Arc<[f32]> = v.into();
    map.insert(sample_rate, arc.clone());
    arc
}

// ---------------------------------------------------------------------------
// FadeEffect
// ---------------------------------------------------------------------------
pub struct FadeEffect {
    enabled: bool,
    duration: Duration,
    audio_format: AudioFormat,
    pos: u32,
    duration_samples: u32,
    pending_pause: bool,
    pause_done: bool,

    // LUT 相关（随 audio_format / duration 变化，不在热循环里重建）
    lut: Arc<[f32]>,
    lut_size: usize,
    lut_scale: f64, // lut_size / duration_samples
}

impl FadeEffect {
    pub fn new() -> Self {
        let mut s = Self {
            enabled: true,
            duration: Duration::from_secs(1),
            audio_format: AudioFormat::default(),
            pos: 0,
            duration_samples: 1,
            pending_pause: false,
            pause_done: false,
            lut: Arc::from(Vec::<f32>::new()),
            lut_size: MIN_LUT_SIZE,
            lut_scale: 1.0,
        };
        s.reset_duration_samples();
        s
    }

    pub fn set_duration(&mut self, duration: Duration) {
        let duration = if duration.is_zero() {
            Duration::from_millis(1)
        } else if duration > Duration::from_secs(60) {
            Duration::from_secs(60)
        } else {
            duration
        };
        self.duration = duration;
        self.reset_duration_samples();
    }

    fn reset_duration_samples(&mut self) {
        // sample_rate == 0 时兜底为 1，避免除零
        let sr = self.audio_format.sample_rate.max(1);
        let ms = u64::try_from(self.duration.as_millis()).unwrap_or(1_000);
        // u64 中间量：96kHz * 60000ms 会溢出 u32
        let n = u64::from(sr) * ms / 1000;
        self.duration_samples = n.clamp(1, u64::from(u32::MAX)) as u32;

        self.lut = get_lut(sr);
        self.lut_size = lut_size_for(sr);
        self.lut_scale = self.lut_size as f64 / f64::from(self.duration_samples);
    }

    /// 热路径查表：f64 索引避免 192kHz*60s (~11.5M) 超 f32 整数精确区
    #[inline]
    fn fade_at(&self, pos: u32) -> f32 {
        let idx = (f64::from(pos) * self.lut_scale) as usize;
        // lut 长度 = lut_size + 1，末位即 pos == duration_samples 时的端点 1.0
        self.lut[idx.min(self.lut_size)]
    }
}

// ---------------------------------------------------------------------------
// PostEffect
// ---------------------------------------------------------------------------
impl PostEffect for FadeEffect {
    fn init(&mut self, audio_format: AudioFormat) {
        self.reset(audio_format);
    }

    fn process(&mut self, buffer: &mut [f32]) {
        let ch = self.audio_format.channels as usize;
        if ch == 0 {
            return;
        }

        if self.pending_pause {
            // 淡出已到 0：整块清零（pause_done 短路避免重复 fill）
            if self.pos == 0 {
                if !self.pause_done {
                    buffer.fill(0.0);
                    self.pause_done = true;
                }
                return;
            }
            let mut pos = self.pos;
            for frame in buffer.chunks_mut(ch) {
                // 尾块可能短于 ch，仍按一帧推进
                pos = pos.saturating_sub(1);
                let f = self.fade_at(pos);
                for v in frame.iter_mut() {
                    *v *= f;
                }
            }
            self.pos = pos;
        } else {
            // 稳态（增益恒为 1）：直接早退，零逐帧开销
            if self.pos == self.duration_samples {
                return;
            }
            let dur = self.duration_samples;
            let mut pos = self.pos;
            for frame in buffer.chunks_mut(ch) {
                pos = (pos + 1).min(dur);
                let f = self.fade_at(pos);
                for v in frame.iter_mut() {
                    *v *= f;
                }
            }
            self.pos = pos;
        }
    }

    fn reset(&mut self, audio_format: AudioFormat) {
        self.audio_format = audio_format;
        self.reset_duration_samples();
    }

    fn transport_hook(&mut self) -> Option<&mut dyn TransportAware> {
        Some(self)
    }

    fn enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn name(&self) -> &'static str {
        "FadeEffect"
    }
}

// ---------------------------------------------------------------------------
// TransportAware
// ---------------------------------------------------------------------------
impl TransportAware for FadeEffect {
    fn on_transport(&mut self, cmd: PlayCmd) -> Decision {
        match cmd {
            // 暂停：先淡出，等 poll_transport 报告完成
            PlayCmd::Pause => {
                self.pending_pause = true;
                self.pause_done = false;
                Decision::Defer
            }
            // 播放：立即恢复 stream，包络自然从当前值淡入
            PlayCmd::Play => {
                self.pending_pause = false;
                self.pause_done = false;
                Decision::Pass
            }
            // Seek：不拦，但复位包络避免残留增益
            PlayCmd::Seek(_) => {
                self.pos = 0;
                self.pending_pause = false;
                self.pause_done = false;
                Decision::Pass
            }
        }
    }

    fn poll_transport(&mut self) -> Option<PlayCmd> {
        if self.pause_done {
            self.pause_done = false;
            Some(PlayCmd::Pause)
        } else {
            None
        }
    }

    fn reset_transport(&mut self) {
        self.pos = 0;
        self.pending_pause = false;
        self.pause_done = false;
    }
}
