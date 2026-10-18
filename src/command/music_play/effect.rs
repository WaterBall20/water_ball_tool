use std::error::Error;
use std::time::Duration;

pub mod fade;
pub mod resampler;

#[derive(Copy, Clone)]
pub struct AudioFormat {
    channels: u16,
    sample_rate: u32,
}
impl Default for AudioFormat {
    fn default() -> Self {
        Self {
            channels: 2,
            sample_rate: 44_100,
        }
    }
}
impl AudioFormat {
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
        }
    }
}

/*pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub format: AudioFormat,
}

impl AudioBuffer {
    pub fn with_format(format: AudioFormat, capacity_frames: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity_frames * format.channels as usize),
            format,
        }
    }
    pub fn frames(&self) -> usize {
        self.samples.len() / self.format.channels as usize
    }
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }
}

impl<I> Index<I> for AudioBuffer
where
    I: SliceIndex<[f32], Output = f32>,
{
    type Output = f32;

    fn index(&self, index: I) -> &Self::Output {
        self.samples.index(index)
    }
}

impl<I> IndexMut<I> for AudioBuffer
where
    I: SliceIndex<[f32], Output = f32>,
{
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        self.samples.index_mut(index)
    }
}
*/
pub trait PostEffect: Send + 'static {
    ///效果器初始化
    fn init(&mut self, format: AudioFormat);
    /// buffer 是交错排列的 f32（interleaved），channels 是声道数
    fn process(&mut self, buffer: &mut [f32]);
    /// 可选：采样率变化、seek 时重置内部状态
    fn reset(&mut self, format: AudioFormat);

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

pub struct PostEffectChain {
    effects: Vec<Box<dyn PostEffect>>,
    format: AudioFormat,
}

impl PostEffectChain {
    pub fn new(format: AudioFormat) -> Self {
        Self {
            effects: Vec::new(),
            format,
        }
    }

    pub fn push<E: PostEffect>(&mut self, e: E) {
        let mut e = e;
        e.init(self.format);
        self.effects.push(Box::new(e));
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        for e in self.effects.iter_mut().filter(|e| e.enabled()) {
            e.process(buffer);
        }
    }

    pub fn reset(&mut self, format: AudioFormat) {
        self.format = format;
        for e in &mut self.effects {
            e.reset(self.format);
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

pub trait PreEffect: Send + 'static {
    ///效果器初始化
    fn init(&mut self, input_format: AudioFormat) -> Result<(), Box<dyn Error>>;
    /// 给定输入格式，声明本级的输出格式（链初始化时用于推导最终格式）
    fn output_format(&self) -> AudioFormat;

    /// 处理 input，结果【追加】到 out。
    /// 调用方保证：out.format 已是 output_format(input.format)，out 已清空。
    /// 允许产出 0 帧（内部缓冲未凑够）或多帧（上采样）。
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize;

    /// 排空内部延迟（重采样器尾部、FIR 缓冲），追加到 out
    fn flush(&mut self, out: &mut Vec<f32>) {}

    /// 重置
    fn reset(&mut self, input_format: AudioFormat);
    /// 对于
    fn try_reset(&mut self, input_format: AudioFormat) -> Result<(), Box<(dyn Error + 'static)>> {
        self.reset(input_format);
        Ok(())
    }

    ///是否启用
    fn enabled(&self) -> bool;

    /// 设置是否启用
    fn set_enabled(&mut self, enabled: bool);
}

pub struct PreEffectChain {
    effects: Vec<Box<dyn PreEffect>>,
    /// 每级的输入格式，初始化时算好，供 process / flush 使用
    input_formats: Vec<AudioFormat>,
    /// 整条链的输出格式
    output_format: AudioFormat,
}

impl PreEffectChain {
    pub fn new(input_format: AudioFormat) -> Self {
        Self {
            effects: Vec::new(),
            input_formats: vec![input_format],
            output_format: input_format,
        }
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn push<E: PreEffect>(&mut self, e: E) -> Result<(), Box<(dyn Error + 'static)>> {
        let mut e = e;
        e.init(self.input_formats[self.input_formats.len() - 1])?;
        let output_format = e.output_format();
        self.effects.push(Box::new(e));
        self.input_formats.push(output_format);
        self.output_format = output_format;
        Ok(())
    }

    pub fn output_format(&self) -> AudioFormat {
        self.output_format
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> usize {
        if self.effects.is_empty() {
            return 0;
        }
        let mut input_temp_buf = input.to_vec();
        let mut output_temp_buf = Vec::with_capacity(input.len().max(output.len()));
        let mut this_len = input.len();
        for effect in &mut self.effects.iter_mut().filter(|e| e.enabled()) {
            this_len = effect.process(&input_temp_buf[..this_len], &mut output_temp_buf);
            if this_len == 0 {
                return 0;
            }
            std::mem::swap(&mut input_temp_buf, &mut output_temp_buf); //交换
        }
        output.clear();
        output.extend_from_slice(&input_temp_buf);
        this_len
    }

    pub fn reset(&mut self, input_format: AudioFormat) {
        self.input_formats[0] = input_format;
        let mut input_format = input_format;
        for (index, effect) in self.effects.iter_mut().enumerate() {
            effect.reset(input_format);
            input_format = effect.output_format();
            self.input_formats[index + 1] = input_format;
        }
        self.output_format = input_format;
    }
}
