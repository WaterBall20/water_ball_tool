use crate::command::music_play::effect::{
    AudioFormat, Decision, PlayCmd, PostEffect, TransportAware,
};
use std::time::Duration;

pub struct FadeEffect {
    enabled: bool,
    duration: Duration,
    audio_format: AudioFormat,
    pos: u32,
    duration_samples: u32,
    pending_pause: bool,
    pause_done: bool,
}

impl FadeEffect {
    pub fn new() -> Self {
        Self {
            enabled: true,
            duration: Duration::from_secs(1),
            audio_format: AudioFormat::default(),
            pos: 0,
            duration_samples: 1,
            pending_pause: false,
            pause_done: false,
        }
    }
    pub fn set_duration(&mut self, duration: Duration) {
        let duration = if duration.is_zero() {
            Duration::from_millis(1)
        } else if duration.as_secs() > 60 {
            Duration::from_secs(60)
        } else {
            duration
        };
        self.duration = duration;
        self.reset_duration_samples();
    }

    fn reset_duration_samples(&mut self) {
        self.duration_samples = self.audio_format.sample_rate
            * u32::try_from(self.duration.as_millis()).unwrap_or(1_000)
            / 1000;
    }
}

impl PostEffect for FadeEffect {
    fn init(&mut self, audio_format: AudioFormat) {
        self.reset(audio_format);
    }

    fn process(&mut self, buffer: &mut [f32]) {
        if !self.pending_pause && self.pos == self.duration_samples {
            return;
        }
        if self.pending_pause && self.pos == 0 {
            buffer.fill(0.0);
            self.pause_done = true;
            return;
        }

        for frame in buffer.chunks_mut(self.audio_format.channels as usize) {
            if self.pending_pause {
                if self.pos > 0 {
                    self.pos = self.pos.saturating_sub(1);
                }
            } else {
                self.pos = (self.pos + 1).min(self.duration_samples);
            }
            let t = (f64::from(self.pos) / f64::from(self.duration_samples)).clamp(0.0, 1.0);
            let fade = 0.5 * (1.0 - (f64::from(std::f32::consts::PI) * t).cos()) as f32;
            for buf_value in frame.iter_mut() {
                *buf_value *= fade;
            }
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
}

impl TransportAware for FadeEffect {
    fn on_transport(&mut self, cmd: PlayCmd) -> Decision {
        match cmd {
            // 暂停：先淡出，延迟执行
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
            // Stop / Seek：不拦，但复位包络避免残留增益
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
