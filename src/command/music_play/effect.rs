use std::time::Duration;

pub mod fade;

pub trait Effect: Send + 'static {
    ///效果器初始化
    fn init(&mut self, channels: u16, sample_rate: u32);
    /// buffer 是交错排列的 f32（interleaved），channels 是声道数
    fn process(&mut self, buffer: &mut [f32]);
    /// 可选：采样率变化、seek 时重置内部状态
    fn reset(&mut self, channels: u16, sample_rate: u32);

    /// 默认：不参与 transport 决策（纯 DSP）
    fn transport_hook(&mut self) -> Option<&mut dyn TransportAware> {
        None
    }

    ///是否启用
    fn enabled(&self) -> bool;

    //设置是否启用
    fn set_enabled(&mut self, enabled: bool);
}

#[derive(Copy, Clone)]
pub enum PlayCmd {
    Play,
    Pause,
    Seek(Duration),
}

/// 效果器对 transport 指令的答复
pub enum Decision {
    Pass,  // 我不关心，控制器立即执行
    Defer, // 我要拦一下，等我 poll 出结果再执行
}

/// 只有声明拦截的效果器才实现这个 trait
pub trait TransportAware: Send {
    fn on_transport(&mut self, cmd: PlayCmd) -> Decision;
    /// 返回 Some(cmd) 表示"我处理完了，可以真正执行 cmd 了"
    fn poll_transport(&mut self) -> Option<PlayCmd>;
    /// seek / 切歌时重置内部包络状态
    fn reset_transport(&mut self) {}
}

pub struct EffectChain {
    effects: Vec<Box<dyn Effect>>,
    channels: u16,
    sample_rate: u32,
}

impl EffectChain {
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            effects: Vec::new(),
            channels,
            sample_rate,
        }
    }

    pub fn push<E: Effect>(&mut self, e: E) {
        let mut e = e;
        e.init(self.channels, self.sample_rate);
        self.effects.push(Box::new(e));
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        for e in &mut self.effects {
            e.process(buffer);
        }
    }

    pub fn reset(&mut self, channels: u16, sample_rate: u32) {
        self.channels = channels;
        self.sample_rate = sample_rate;
        for e in &mut self.effects {
            e.reset(self.channels, self.sample_rate);
        }
    }

    /// 广播 transport 指令，汇总决策
    pub fn dispatch_transport(&mut self, cmd: PlayCmd) -> Decision {
        let mut deferred = false;
        for e in &mut self.effects {
            if e.enabled()
                && let Some(h) = e.transport_hook()
                && matches!(h.on_transport(cmd), Decision::Defer)
            {
                deferred = true;
            }
        }
        if deferred {
            Decision::Defer
        } else {
            Decision::Pass
        }
    }

    /// 收集已完成、需要真正执行的指令
    pub fn poll_transport(&mut self, out: &mut Vec<PlayCmd>) {
        for e in &mut self.effects {
            if e.enabled()
                && let Some(h) = e.transport_hook()
                && let Some(done) = h.poll_transport()
            {
                out.push(done);
            }
        }
    }
}
