use std::error::Error;
use std::time::Duration;

pub mod cross_bridge;
pub mod fade;
pub mod resampler;
pub mod runtime;
#[cfg(test)]
pub mod test;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
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

    /// 设置是否启用
    fn set_enabled(&mut self, enabled: bool);
    /// 名称
    fn name(&self) -> &str;
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
/// 非实时音频前处理器（在独立线程中运行）。
///
/// # 生命周期
///
/// ```text
/// init → [ reset → process* → input_eof ]* → input_none? → drop
/// ```
///
/// - `init` / `output_format`：加入链之前调用一次并查询。
/// - `reset`：每首歌数据开始前调用。
/// - `process`：反复调用，直到本曲数据耗尽。
/// - `input_eof`：本曲数据结束时调用一次，负责吐出 tail。
/// - `input_none`：无下一曲或立刻换曲时调用，执行彻底复位。
///
/// # 通用约定
///
/// - **样本格式**：交错 f32，长度单位为「样本数」（含声道）。
/// - **输入**：`input` 必须被全部消费，管理器不保留任何跨调用残留。
///   若实现需要流式部分消费，必须自己保存残留并在下次调用先处理。
/// - **输出缓冲**：`out` 由调用方持有并复用，**调用方负责在调用前 clear**。
///   实现只允许 `clear / extend / push / resize`，
///   **不得替换 `out` 本身、不得 `shrink_to_fit`、不得与内部缓冲 swap**。
/// - **返回值**：`process` 与 `input_eof` 均返回 **`out.len()`**。
/// - **错误**：`process` 不允许失败或 panic；可失败的操作在 `init` /
///   `try_reset` 中完成并上报。
/// - **enabled**：仅控制数据流是否经过本级，**不控制生命周期回调**。
pub trait PreEffect: Send + 'static {
    // ======================================================================
    // 初始化与格式协商
    // ======================================================================

    /// 初始化。在加入效果器组之前调用。
    ///
    /// 调用方保证：之后才查询 [`output_format`](Self::output_format)，
    /// 并按返回值对下一级 `init`。重复调用等价于 `reset` + 重新配置。
    ///
    /// 失败时该效果器不应被加入链中。
    ///
    /// 错误类型为 `Send + Sync`，以便从独立线程回传到主线程。
    fn init(
        &mut self,
        input_format: AudioFormat,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>>;

    /// 返回本级的输出格式，作为下一级的输入格式。
    ///
    /// 必须在 `init` 成功之后调用。
    ///
    /// # 固定性
    ///
    /// 一旦协商完成，`output_format` **不得再改变**：
    /// `reset` / `input_none` / toggle `enabled` 均不得改变它。
    /// 若输入格式发生变化，由管理器重建整条链，而不是依赖本方法重协商。
    fn output_format(&self) -> AudioFormat;

    // ======================================================================
    // 数据处理
    // ======================================================================

    /// 处理 `input`，结果写入 `out`。
    ///
    /// # 参数
    ///
    /// - `input`：交错 f32 样本。支持任意长度（含空），不保证是块大小的整数倍。
    ///   必须被全部消费。
    /// - `out`：调用方已 `clear` 的复用缓冲。实现只追加，不替换、不缩容。
    ///
    /// # 返回
    ///
    /// 返回写入 `out` 的样本数，即返回后 `out.len()`。
    ///
    /// # 返回 0 的语义
    ///
    /// - **不跳过下级**：管理器仍会调用下一级，其 `input` 为空切片。
    /// - 收到空 `input` 时，`out` **必须为空**，返回 0。
    ///   不得借空输入吐出 tail —— tail 只能在 [`input_eof`](Self::input_eof) 吐。
    /// - 有状态/时间敏感的下级在收到空输入时不会推进时间线；
    ///   若本级需要「暂时静音但保持时间线」，应输出静音样本而非返回 0。
    ///
    /// # 禁止
    ///
    /// - 不得部分消费 `input`（残留不会传播到下级）。
    /// - 不得在 `process` 中失败或 panic。
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize;

    // ======================================================================
    // EOF / 复位
    // ======================================================================

    /// 本曲数据已结束，输出本曲结尾数据（含本级 tail）。
    ///
    /// 每首歌调用一次。调用后到下次 `reset` 前不应再调用 `process`。
    ///
    /// # 参数
    ///
    /// - `input`：上一级的输出，可能为空。
    ///   - 第一级：恒为空（解码器保证 `process` 已送完全部数据）。
    ///   - 后续级：上级在 `input_eof` 吐出的数据，可能为空。
    ///
    /// 实现应将 `input` **按正常数据处理**，处理结果写入 `out`，
    /// 随后**追加本级内部 tail**。默认实现（拷贝 `input` 到 `out`）
    /// 对无 tail 的纯处理型效果器是正确的。
    ///
    /// # 返回
    ///
    /// 返回写入 `out` 的样本数，即返回后 `out.len()`。
    ///
    /// # 返回 0 的语义
    ///
    /// **不跳过下级**：本级无 tail 不代表下级无 tail，管理器仍会调用下一级。
    fn input_eof(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        out.clear();
        out.reserve_exact(input.len());
        out.extend_from_slice(input);
        input.len()
    }

    /// 无下一曲，或立刻换曲。执行**彻底复位**。
    ///
    /// # 语义
    ///
    /// - 清空**全部处理状态**：延迟线、滤波器历史、卷积/混响 tail、
    ///   重采样残留、LFO 相位、lookahead 缓冲等。
    /// - **保留配置**：增益、EQ 曲线、阈值/压缩比等参数以**最新值**为准，
    ///   **不得回滚到 `init` 时的初始值**。
    /// - 复位后等价于「以**当前最新配置**重新 `init` 完成」的状态。
    /// - 输入格式不改变，`output_format` 不得改变。
    ///
    /// # 调用顺序
    ///
    /// **无顺序保证**：可能在 `reset` 之前、`process` 中途、`input_eof` 之后
    /// 或从未 `reset` 过时被调用。实现**不得假设任何前置状态**，
    /// 也不得依赖「`reset` 保存的快照」来复位。
    ///
    /// # enabled
    ///
    /// 即使本级 `enabled == false` 也会被调用，必须完整执行。
    fn input_none(&mut self) {
        // 默认空实现：无内部状态的效果器无需处理
    }

    /// 每首歌数据开始前调用。执行**基本复位**。
    ///
    /// # 语义
    ///
    /// 清理跨曲不应保留的处理状态，但**可以保留**无需重建的缓存
    /// （如已分配的缓冲、格式相关的预计算、滤波器系数）。
    /// 配置参数保持不变。
    ///
    /// # 约束
    ///
    /// - 不得改变 `output_format`。
    /// - 即使本级 `enabled == false` 也会被调用，必须执行，不得依赖 `enabled` 状态。
    fn reset(&mut self, input_format: AudioFormat);

    /// 可失败的复位。**预留接口**：将来会替换 [`reset`](Self::reset)，
    /// 目前不被效果管理器调用。
    ///
    /// 默认实现委托给 `reset` 并返回 `Ok(())`。
    fn try_reset(
        &mut self,
        input_format: AudioFormat,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        self.reset(input_format);
        Ok(())
    }

    // ======================================================================
    // 状态查询与开关
    // ======================================================================

    /// 是否启用。
    ///
    /// 返回 `false` 时：
    /// - `process` **不被调用**，管理器将 `input` 直接透传给下一级；
    /// - `input_eof` 仍被调用，但 `input` 为空、`out` 被忽略（仅作清理通知）；
    /// - `reset` / `input_none` / `try_reset` 仍被调用，必须完整执行。
    ///
    /// 本级的 `output_format` 不因 `enabled` 变化而改变。
    fn enabled(&self) -> bool;

    /// 设置是否启用。运行时 toggle 不得改变 `output_format`。
    fn set_enabled(&mut self, enabled: bool);

    /// 名称。预留给管理器和调试用。
    fn name(&self) -> &str;

    // ======================================================================
    // 可选：时间信息（有默认实现，管理器用于预分配与时间戳对齐）
    // ======================================================================

    /// 本级内部 tail 的样本数（含声道）。
    ///
    /// 用于 `input_eof` 前给 `out` 预分配，避免长 tail（卷积混响等）
    /// 触发反复扩容。无 tail 的效果器返回 0。
    fn tail_samples(&self) -> usize {
        0
    }

    /// 本级处理延迟的样本数（含声道）。
    ///
    /// 用于离线预处理后的时间戳对齐、seek、进度同步。
    /// 无延迟的效果器返回 0。
    fn latency_samples(&self) -> usize {
        0
    }
}

pub struct PreEffectChain {
    effects: Vec<Box<dyn PreEffect>>,
    /// 每级的输入格式，初始化时算好，供 process / flush 使用
    input_formats: Vec<AudioFormat>,
    /// 整条链的输出格式
    output_format: AudioFormat,
    input_buffer: Vec<f32>,
    output_buffer: Vec<f32>,
    no_buffer: Vec<f32>, //忽略的缓冲区
}

impl PreEffectChain {
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
            input_formats: vec![AudioFormat::default()],
            output_format: AudioFormat::default(),
            input_buffer: vec![],
            output_buffer: vec![],
            no_buffer: vec![],
        }
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn push<E: PreEffect>(
        &mut self,
        e: E,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
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
        //效果器不存在就复制数据
        if self.effects.is_empty() {
            output.clear();
            output.reserve_exact(input.len());
            output.extend_from_slice(input);
            return input.len();
        }
        let input_temp_buf = &mut self.input_buffer;
        input_temp_buf.clear();
        input_temp_buf.reserve_exact(input.len());
        input_temp_buf.extend_from_slice(input);
        let output_temp_buf = &mut self.output_buffer;
        let mut this_len = input.len();
        for effect in &mut self.effects.iter_mut().filter(|e| e.enabled()) {
            output_temp_buf.clear();
            this_len = effect.process(&input_temp_buf[..this_len], output_temp_buf);
            std::mem::swap(input_temp_buf, output_temp_buf); //交换
        }
        output.clear();
        output.extend_from_slice(input_temp_buf);
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

    pub fn input_eof(&mut self, out: &mut Vec<f32>) -> usize {
        let input_buf = &mut self.input_buffer;
        let output_buf = &mut self.output_buffer;
        let no_buf = &mut self.no_buffer;
        let mut this_len = 0;
        for effect in &mut self.effects {
            if effect.enabled() {
                output_buf.clear();
                this_len = effect.input_eof(&input_buf[..this_len], output_buf);
                std::mem::swap(input_buf, output_buf);
            } else {
                effect.input_eof(&no_buf[..0], output_buf);
            }
        }
        out.clear();
        out.reserve_exact(this_len);
        out.extend_from_slice(&input_buf[..this_len]);
        this_len
    }

    pub fn input_none(&mut self) {
        for effect in &mut self.effects {
            effect.input_none();
        }
    }
}
