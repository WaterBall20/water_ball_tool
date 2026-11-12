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
    #[must_use]
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
        }
    }

    #[must_use]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    //AI=== 交错样本数换算时长：供调试页估算缓冲区延迟 / convert interleaved samples to a duration for the debug latency gauges
    /// 把交错样本数（含声道）换算为时长 / convert interleaved samples (channels included) to a duration
    #[must_use]
    pub fn samples_to_duration(&self, samples: usize) -> Duration {
        let samples_per_second = f64::from(self.sample_rate) * f64::from(self.channels);
        if samples == 0 || samples_per_second <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(samples as f64 / samples_per_second)
        }
    }
    //AI_END===
}

//AI=== 静态禁用策略与复位原因 / static disable policy and reset reason
/// 效果器是否允许被用户禁用。必须是静态策略，不能依赖播放状态。
///
/// Whether the user may disable an effect. This MUST be a static policy and must
/// not depend on playback state.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum DisablePolicy {
    Allowed,
    Forbidden,
}

/// `reset` 的调用原因，供效果器区分新曲、输入格式变化与开关切换。
///
/// Why `reset` was called, so an effect can distinguish a new track, an input
/// format change and an enable/disable toggle.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum ResetReason {
    /// 新曲开始 / a new track starts
    TrackStart,
    /// 上游格式或信号链路变化 / the upstream format or signal chain changed
    InputChanged,
    /// 用户切换启用状态 / the user toggled the enabled state
    Toggled,
}
//AI_END===

pub trait PostEffect: Send + 'static {
    ///效果器初始化
    fn init(&mut self, format: AudioFormat);
    /// buffer 是交错排列的 f32（interleaved），channels 是声道数
    fn process(&mut self, buffer: &mut [f32]);
    /// 可选：采样率变化、seek 时重置内部状态
    fn reset(&mut self, format: AudioFormat, reason: ResetReason);

    /// 默认：不参与 transport 决策（纯 DSP）
    fn transport_hook(&mut self) -> Option<&mut dyn TransportAware> {
        None
    }

    ///是否启用
    fn enabled(&self) -> bool;

    /// 设置是否启用
    fn set_enabled(&mut self, enabled: bool);

    //AI=== 静态禁用策略：后效果器严格同步，默认允许直接切换 / static disable policy; post effects are synchronous and toggle directly
    /// 是否允许用户禁用。默认允许；带内部延迟的后效果器应返回 `Forbidden`。
    ///
    /// Whether the user may disable this effect. Allowed by default; a post effect
    /// with internal latency must return `Forbidden`.
    fn disable_policy(&self) -> DisablePolicy {
        DisablePolicy::Allowed
    }
    //AI_END===

    /// 名称
    fn name(&self) -> &str;

    //AI=== 给人看的效果器名称与介绍，供效果器窗口列表使用 / human-readable name and description for the effects window
    /// 给人看的效果器名称 / human-readable effect name
    fn display_name(&self) -> &'static str;
    /// 效果器简介 / effect description
    fn description(&self) -> &'static str;
    //AI_END===
}

//AI=== 效果器窗口的数据行：只由 trait 方法生成 / effects-window rows, built from trait methods only
/// 效果器所在的链 / which chain an effect belongs to
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum EffectChainKind {
    Pre,
    Post,
}

/// 效果器只读展示快照 / read-only effect display snapshot
#[derive(Clone, Debug)]
pub struct EffectSummary {
    pub chain: EffectChainKind,
    /// 在本链中的索引，用于回写启用状态 / index within the chain, used to write the enabled state back
    pub index: usize,
    pub name: &'static str,
    pub description: &'static str,
    pub enabled: bool,
    //AI=== 静态禁用策略，供 UI 显示与拒绝切换 / static disable policy for display and toggle rejection
    /// 是否允许用户禁用 / whether the user may disable this effect
    pub can_disable: bool,
    //AI_END===
    /// 前效果器内部延迟线的当前时长；后效果器为 0 / current pre-effect delay-line duration; 0 for post effects
    pub latency: Duration,
}
//AI_END===

//AI=== 启用/禁用链级操作的失败原因 / failure reason of a chain-level enable/disable
/// 链级启用/禁用失败 / chain-level toggle failure
#[derive(Debug)]
pub enum EffectToggleError {
    /// 索引无效 / invalid effect index
    InvalidIndex,
    /// 静态策略禁止禁用 / the static policy forbids disabling
    Forbidden,
    /// 部分效果器 reset 失败，已降级为禁用 / some effects failed to reset and were degraded to disabled
    Reset(Vec<(PreEffectInfo, PreEffectError)>),
}
//AI_END===

#[derive(Copy, Clone, PartialEq)]
pub enum PlayCmd {
    Play,
    Pause,
    Seek(Duration),
    ClearEofBuf,
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
    #[must_use]
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

    pub fn reset(&mut self, format: AudioFormat, reason: ResetReason) {
        self.format = format;
        for e in &mut self.effects {
            e.reset(self.format, reason);
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
    ///
    /// 这里不按 enabled 过滤：淡出期间被禁用的效果器仍可能持有一个已 defer 的
    /// 暂停指令，必须让它完成，否则暂停永远不会确认。
    ///
    /// Disabled effects are polled too: an effect disabled during a fade may still
    /// hold a deferred pause command, which must be released instead of hanging.
    pub fn poll_transport(&mut self, out: &mut Vec<PlayCmd>) {
        for e in &mut self.effects {
            if let Some(h) = e.transport_hook()
                && let Some(done) = h.poll_transport()
            {
                out.push(done);
            }
        }
    }

    //AI=== 后效果器列表：只读 trait 方法，新增效果器自动出现 / post-effect list: read-only trait methods, new effects appear automatically
    /// 后效果器的展示快照 / display snapshot of the post-effect chain
    #[must_use]
    pub fn effect_summaries(&self) -> Vec<EffectSummary> {
        self.effects
            .iter()
            .enumerate()
            .map(|(index, effect)| EffectSummary {
                chain: EffectChainKind::Post,
                index,
                name: effect.display_name(),
                description: effect.description(),
                enabled: effect.enabled(),
                can_disable: effect.disable_policy() == DisablePolicy::Allowed,
                latency: Duration::ZERO,
            })
            .collect()
    }
    //AI_END===

    //AI=== 后效果器启用/禁用：严格同步，直接切换 / post effects are synchronous, so toggle directly
    /// 设置指定后效果器的启用状态；索引无效或策略禁止时返回 false。
    ///
    /// Set the enabled state of a post effect; returns false for an invalid index or a
    /// forbidden static policy.
    pub fn set_effect_enabled(&mut self, index: usize, enabled: bool) -> bool {
        let Some(effect) = self.effects.get_mut(index) else {
            return false;
        };
        if !enabled && effect.disable_policy() == DisablePolicy::Forbidden {
            return false;
        }
        effect.set_enabled(enabled);
        true
    }
    //AI_END===
}

pub enum AudioData {
    /// 数据块
    Block(Vec<f32>),
    /// 重置
    Reset(AudioFormat),
    /// 解码EOF
    Eof,
    /// 没有
    NoNext,
    /// 跳过
    Skip,
    /// 更改进度
    Seek,
}

// ============================================================================
// 基础类型
// ============================================================================

/// 前处理器错误类型。要求 `Send + Sync`，以便从独立线程回传到主线程。
pub type PreEffectError = Box<dyn Error + Send + Sync + 'static>;

/// 音频流的不连续点类型。
///
/// 与 `process`（连续数据热路径）分离，避免冷分支进入热路径。
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PreEffectDiscontinuity {
    /// 有下一曲，本曲正常结束。
    ///
    /// 处理 `input`（上级 tail）+ 追加本级 tail，不复位。
    /// 随后管理器对本级 `reset` 开始下一首。
    Eof,

    /// 无下一曲（播放列表结束），本曲正常结束。
    ///
    /// 处理 `input` + 追加本级 tail（可更长淡出），然后彻底复位。
    /// 随后结束播放。
    NoNext,

    /// 立刻换曲 / 停止 / 跳过。
    ///
    /// 丢弃 `input`，不追加 tail，彻底复位。`out` 为空，返回 0。
    Skip,

    /// 同一首歌进度跳转（seek）。
    ///
    /// 丢弃 `input`（旧位置数据无意义），不追加 tail，
    /// 执行基本复位（等价 `reset` 的复位逻辑，但**不得改变格式**）。
    /// `out` 为空，返回 0。随后从新位置**立即** `process`，
    /// 管理器不 `reset`、不重建。
    Seek,

    //AI=== 启用/禁用迁移：目标级排空，下游收到链路变化通知 / toggle transition: target drains, downstream is notified
    /// 本级即将被禁用。
    ///
    /// 把内部仍应播放的样本追加到 `out`，并自行决定轻量复位程度；
    /// 不得改变格式。管理器随后调用 `set_enabled(false)`。
    DisableSelf,

    /// 上游某级被禁用，`input` 是上游排出的数据。
    ///
    /// `distance` 为距被禁用级的链位置：1 = 直接前者，2 = 前前者，以此类推。
    /// 实现应消费 `input`（可透传），自行决定是否复位内部状态。
    UpstreamDisabled { distance: usize },
    //AI_END===
}

impl TryFrom<&AudioData> for PreEffectDiscontinuity {
    type Error = Box<dyn Error + Send + Sync + 'static>;

    fn try_from(value: &AudioData) -> Result<Self, Self::Error> {
        Ok(match value {
            AudioData::Block(_) | AudioData::Reset(_) => return Err("类型不支持".into()),
            AudioData::Eof => Self::Eof,
            AudioData::NoNext => Self::NoNext,
            AudioData::Skip => Self::Skip,
            AudioData::Seek => Self::Seek,
        })
    }
}

// ============================================================================
// 返回值结构
// ============================================================================

/// `init` 和 `reset` 的统一返回：格式协商结果。
///
/// 管理器据此链式传递格式：
/// 第 i 级的 `output_format` 即第 i+1 级的 `input_format`。
///
/// 只含调用后可能变化、且管理器需要的字段。
/// 完整信息（name / enabled / latency / tail 等）通过
/// [`PreEffect::info`] 查询。
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct PreEffectOutputFormat {
    /// 本级对本次 `input_format` 的响应输出格式。
    pub output_format: AudioFormat,
}

/// 完整信息快照。由 [`PreEffect::info`] 返回引用。
///
/// 只承载**结构性低频信息**。运行时参数调优走独立 setter /
/// `PreEffectControl` 等控制接口，**不修改本结构体字段**，
/// 以便后续扩展控制能力时不产生破坏性变更。
#[derive(Clone, Debug)]
pub struct PreEffectInfo {
    /// 名称
    pub name: String,
    /// 是否启用。由 `set_enabled` 同步更新。
    pub enabled: bool,
    /// 初始化/最近一次 reset 时记录的输入格式。
    pub input_format: AudioFormat,
    /// 当前输出格式。
    pub output_format: AudioFormat,
    /// 本级处理延迟的样本数（含声道），用于时间戳 / seek 对齐。
    pub latency_samples: usize,
    /// 本级内部 tail 的样本数（含声道），用于预分配 `out`。
    pub tail_samples: usize,
}

// ============================================================================
// trait
// ============================================================================

/// 非实时音频前处理器（在独立线程中运行）。
///
/// # 生命周期
///
/// ```text
/// init → [ reset → process* → on_discontinuity ]* → drop
/// ```
///
/// - `init`：加入链之前调用**一次**，之后不再调用。
/// - `reset`：每首歌数据开始前调用一次。
/// - `process`：反复调用，直到本曲数据耗尽。
/// - `on_discontinuity`：本曲结束 / 中断 / seek 时调用。
///
/// 效果器实例生命周期贯穿整个播放过程，**不销毁、不重建**。
/// 格式变化完全由效果器在 `reset` 内部消化。
///
/// # 通用约定
///
/// - **样本格式**：交错 f32，长度单位为「样本数」（含声道）。
/// - **输入**：`input` 必须被全部消费，管理器不保留任何跨调用残留。
/// - **输出缓冲**：`out` 由调用方持有并复用，**调用方负责在调用前 clear**。
///   实现只允许 `clear / extend / push / resize`，
///   **不得替换 `out` 本身、不得 `shrink_to_fit`、不得与内部缓冲 swap**。
/// - **返回值**：`process` 与 `on_discontinuity` 均返回 **`out.len()`**。
/// - **错误**：`process` 不允许失败或 panic；可失败操作在 `init` / `reset` 中完成。
/// - **enabled**：仅控制数据流是否经过本级，**不控制生命周期回调**。
pub trait PreEffect: Send + 'static {
    // ========================================================================
    // 初始化与格式协商
    // ========================================================================

    /// 初始化。加入链之前调用**一次**，之后不再调用。
    ///
    /// # 参数
    ///
    /// - `input_format`：本级的输入格式，由上一级的 `output_format` 决定
    ///   （第一级由解码器决定）。
    ///
    /// # 返回
    ///
    /// 返回本级的输出格式，作为下一级 `init` 的 `input_format`。
    ///
    /// # 失败
    ///
    /// 失败时该效果器不应被加入链中。
    ///
    /// # 确定性
    ///
    /// 同一 `input_format` + 同一配置下，产出的 `output_format` 必须一致，
    /// 避免调试时行为不可复现。
    fn init(&mut self, input_format: AudioFormat) -> Result<PreEffectOutputFormat, PreEffectError>;

    /// 每首歌数据开始前调用一次，也会在启用/禁用链路变化时调用。
    ///
    /// `reason` 用于区分新曲、输入格式变化与开关切换；实现不得把
    /// `TrackStart` 的“保留上一首 tail”逻辑套用到后两种原因上。
    ///
    /// Called before each track, and also when an enable/disable transition changes
    /// the chain. `reason` distinguishes a new track from a format change or toggle;
    /// implementations must not apply the `TrackStart` tail-preserving behavior to
    /// the other reasons.
    ///
    /// # 语义
    ///
    /// - 对比内部记录的 `input_format` 与传入值：
    ///   - **相同**：仅执行基本复位（清处理状态，保留缓冲/配置）。
    ///   - **不同**：先重建内部资源（缓冲分配、系数预计算、结构重配置等），
    ///     再执行基本复位。重建后必须保证**立即能 `process` 新格式数据**。
    /// - 返回的 `output_format` 作为下一级 `reset` 的 `input_format`。
    /// - 效果器实例**不销毁**，配置保留最新值（不回滚到 `init` 时的初始值）。
    ///
    /// # 失败
    ///
    /// 内部重建失败时返回 `Err`。管理器将该级**降级为透传**
    /// （视同 disabled，`output_format = input_format`），
    /// 继续向下游传递格式，错误上报 UI。本曲不因单级失败终止。
    fn reset(
        &mut self,
        input_format: AudioFormat,
        reason: ResetReason,
    ) -> Result<PreEffectOutputFormat, PreEffectError>;

    // ========================================================================
    // 数据处理
    // ========================================================================

    /// 处理 `input`，结果写入 `out`。**连续数据热路径**。
    ///
    /// # 参数
    ///
    /// - `input`：交错 f32 样本。支持任意长度（含空），不保证是块大小的整数倍。
    ///   **必须被全部消费**；管理器不保留任何跨调用残留。
    /// - `out`：调用方已 `clear` 的复用缓冲。实现只追加，
    ///   **不得替换 / shrink / 与内部缓冲 swap**。
    ///
    /// # 返回
    ///
    /// 返回写入 `out` 的样本数，即返回后 `out.len()`。
    ///
    /// # 返回 0 的语义
    ///
    /// - **不跳过下级**：管理器仍会调用下一级，其 `input` 为空切片。
    /// - 收到空 `input` 时，`output` **必须为空**，返回 0。
    ///   不得借空输入吐出 tail —— tail 只能在 `on_discontinuity` 吐。
    /// - 有状态/时间敏感的下级在收到空输入时不会推进时间线；
    ///   若本级需要「暂时静音但保持时间线」，应输出静音样本而非返回 0。
    ///
    /// # 禁止
    ///
    /// - 不得部分消费 `input`（残留不会传播到下级）。
    /// - 不得在 `process` 中失败或 panic。
    #[must_use]
    fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> usize;

    // ========================================================================
    // 不连续点处理
    // ========================================================================

    /// 处理音频流的不连续点。**离散边界事件**，与 `process` 分离。
    ///
    /// 把边界逻辑塞进 `process` 会让热路径背上分支和冷代码，
    /// 也会让「空输入」和「不连续点」两种语义混淆。
    ///
    /// # 触发时机
    ///
    /// - `Eof`    ：本曲正常结束且**有下一曲**，调用一次。
    /// - `NoNext` ：本曲正常结束且**无下一曲**，调用一次。
    /// - `Skip`   ：用户手动切歌 / 停止 / 跳过，调用一次。
    /// - `Seek`   ：同一首歌内**可多次调用**（每次 seek 都触发）。
    ///
    /// # 参数
    ///
    /// - `input`：上级输出，可能为空。
    ///   - 第一级：恒为空（解码器保证 `process` 已送完全部数据）。
    ///   - 后续级：上级在本方法中吐出的数据。
    /// - `out`：调用方已 `clear` 的复用缓冲。实现只追加，不替换、不缩容。
    /// - `kind`：不连续点类型，决定行为。
    ///
    /// # 返回值
    ///
    /// 返回写入 `out` 的样本数，即返回后 `out.len()`。
    /// **返回 0 不跳过下级**（下级可能有自己的 tail 需要吐）。
    ///
    /// # 各 kind 的行为
    ///
    /// | kind     | 处理 input | 追加 tail | 复位程度 | 返回值       |
    /// |----------|-----------|----------|---------|-------------|
    /// | `Eof`    | 是        | 是       | 不复位   | `out.len()` |
    /// | `NoNext` | 是        | 是（更长）| 彻底复位 | `out.len()` |
    /// | `Skip`   | 丢弃      | 否       | 彻底复位 | 0           |
    /// | `Seek`   | 丢弃      | 否       | 基本复位 | 0           |
    /// | `DisableSelf` | 丢弃 | 是（排空）| 由实现决定 | `out.len()` |
    /// | `UpstreamDisabled` | 是 | 由实现决定 | 由实现决定 | `out.len()` |
    ///
    /// # 复位语义
    ///
    /// - **基本复位**（`Seek`）：清位置相关状态（延迟线、滤波器历史、tail、
    ///   相位、lookahead 等），**保留配置与已分配缓冲**，
    ///   保证立即能 `process` 新位置数据。等价于 `reset` 的复位逻辑，
    ///   但**不得改变 `output_format`**。
    /// - **彻底复位**（`NoNext` / `Skip`）：清全部处理状态，
    ///   配置保留最新值（不回滚到 `init` 初始值），可释放可重建资源。
    ///
    /// # 实现建议
    ///
    /// - `Eof` / `NoNext`：复用 `self.process(input, out)` 处理上级 tail，
    ///   再追加本级 tail。
    /// - `Skip` / `Seek`：走内部 `full_reset` / `basic_reset`，
    ///   两者可与 `reset` 共享复位逻辑。
    ///
    /// 默认实现：无状态效果器直接透传 `input`（`Skip` / `Seek` 输出空）。
    #[must_use]
    fn on_discontinuity(
        &mut self,
        input: &[f32],
        output: &mut Vec<f32>,
        kind: PreEffectDiscontinuity,
    ) -> usize {
        match kind {
            PreEffectDiscontinuity::Skip
            | PreEffectDiscontinuity::Seek
            | PreEffectDiscontinuity::DisableSelf => 0,
            PreEffectDiscontinuity::Eof
            | PreEffectDiscontinuity::NoNext
            | PreEffectDiscontinuity::UpstreamDisabled { .. } => {
                output.extend_from_slice(input);
                output.len()
            }
        }
    }

    // ========================================================================
    // 信息与状态
    // ========================================================================

    /// 完整信息快照。效果器内部持有权威副本。
    ///
    /// `reset` 若更新了 `output_format`，同步更新内部 `PreEffectInfo` 的对应字段。
    #[must_use]
    fn info(&self) -> &PreEffectInfo;

    /// 是否启用。**管理器控制面直接查询此方法**，不经 info()。
    ///
    /// 返回 `false` 时：
    /// - `process` **不被调用**，管理器将 input 直接透传给下一级；
    /// - `on_discontinuity` 仍被调用（复位照常执行，tail 输出被忽略）；
    /// - `reset` 仍被调用，且必须完整执行。
    ///
    /// 与 `set_enabled` 构成唯一权威来源，不和 `info()` 重复。
    #[must_use]
    fn enabled(&self) -> bool;

    /// 设置是否启用。运行时 toggle **不得改变 output_format**。
    fn set_enabled(&mut self, enabled: bool);

    //AI=== 静态禁用策略：会改变格式的效果器必须完全禁止禁用 / a format-changing effect must be statically non-disableable
    /// 是否允许用户禁用。默认允许；会改变输出格式的效果器必须返回 `Forbidden`，
    /// 否则旁路后下游与设备格式不一致。
    ///
    /// Whether the user may disable this effect. Allowed by default; an effect that
    /// changes its output format must return `Forbidden`, otherwise bypassing it would
    /// leave downstream stages and the output device on the wrong format.
    fn disable_policy(&self) -> DisablePolicy {
        DisablePolicy::Allowed
    }
    //AI_END===

    //AI=== 给人看的效果器名称与介绍，供效果器窗口列表使用 / human-readable name and description for the effects window
    /// 给人看的效果器名称 / human-readable effect name
    fn display_name(&self) -> &'static str;
    /// 效果器简介 / effect description
    fn description(&self) -> &'static str;
    //AI_END===

    /// 最小输入工作大小（含声道的交错样本数）。
    ///
    /// 这是给解码器的批量写入提示，不是“低于该值就一定不会工作”的保证：是否产生输出取决于效果器实现。
    /// 没有内部缓冲的效果器可能在小输入上立即输出；有内部缓冲的效果器可能积累到该大小附近才开始输出。
    /// 返回 0 表示解码器无需累积，可以每个包直接写入。
    ///
    /// Minimum input work size (interleaved samples, channels included). This is only a batching hint for the
    /// decoder, not a guarantee that smaller inputs produce no output: that depends on the effect implementation.
    /// An effect without internal buffering may emit immediately; an effect with internal buffering may wait until
    /// roughly this size. `0` means the decoder need not accumulate and may push every packet.
    #[must_use]
    fn min_input_len_work(&self) -> usize {
        0
    }

    /// 当前内部延迟线保留的样本数（本级输入格式，含声道）。
    ///
    /// 仅用于调试显示；管理器按登记的本级输入格式换算，不读取 [`Self::info`]。
    /// 默认 0 表示没有内部缓冲。
    ///
    /// Samples currently retained by the internal delay line in this stage's input format, channels included.
    /// Used for debug display only; the chain converts with its registered input format, never [`Self::info`].
    /// The default 0 means no internal buffering.
    //AI=== 内部延迟线样本数：管理器只信任登记格式 / internal delay-line samples; the chain trusts only registered formats
    #[must_use]
    fn buffered_samples(&self) -> usize {
        0
    }
    //AI_END===
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
    #[must_use]
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

    #[must_use]
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn push<E: PreEffect>(
        &mut self,
        e: E,
    ) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        let mut e = e;
        let output_format = e.init(self.input_formats[self.input_formats.len() - 1])?;
        self.effects.push(Box::new(e));
        self.input_formats.push(output_format.output_format);
        self.output_format = output_format.output_format;
        Ok(())
    }

    #[must_use]
    pub fn output_format(&self) -> AudioFormat {
        self.output_format
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) -> usize {
        //效果器不存在就复制数据
        if self.effects.is_empty() {
            output.clear();
            output.extend_from_slice(input);
            return input.len();
        }
        let input_temp_buf = &mut self.input_buffer;
        input_temp_buf.clear();
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

    pub fn reset(
        &mut self,
        input_format: AudioFormat,
        reason: ResetReason,
    ) -> Result<(), Vec<(PreEffectInfo, PreEffectError)>> {
        let mut errors = Vec::new();
        self.input_formats[0] = input_format;
        let mut input_format = input_format;
        for index in 0..self.effects.len() {
            //AI=== 禁用效果器仍然 reset，但它的输出格式被忽略 / a disabled effect still resets, but its output format is ignored
            let effective = match self.effects[index].reset(input_format, reason) {
                Ok(output) => {
                    if self.effects[index].enabled() {
                        output.output_format
                    } else {
                        input_format
                    }
                }
                Err(e) => {
                    self.effects[index].set_enabled(false);
                    errors.push((self.effects[index].info().clone(), e));
                    input_format
                }
            };
            //AI_END===
            input_format = effective;
            self.input_formats[index + 1] = input_format;
        }
        self.output_format = input_format;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn on_discontinuity(
        &mut self,
        output: &mut Vec<f32>,
        discontinuity: PreEffectDiscontinuity,
    ) -> usize {
        //效果器不存在就直接返回
        if self.effects.is_empty() {
            return 0;
        }
        let input_buf = &mut self.input_buffer;
        let output_buf = &mut self.output_buffer;
        let no_buf = &mut self.no_buffer;
        let mut this_len = 0;
        for effect in &mut self.effects {
            if effect.enabled()
                && (discontinuity == PreEffectDiscontinuity::Eof
                    || discontinuity == PreEffectDiscontinuity::NoNext)
            {
                output_buf.clear();
                this_len =
                    effect.on_discontinuity(&input_buf[..this_len], output_buf, discontinuity);
                std::mem::swap(input_buf, output_buf);
            } else {
                //AI=== 禁用效果器的 tail 输出按约定忽略，但保留 must_use 检查 / the disabled effect's tail output is ignored by contract
                let _ = effect.on_discontinuity(&no_buf[..0], output_buf, discontinuity);
                //AI_END===
            }
        }
        output.clear();
        output.extend_from_slice(&input_buf[..this_len]);
        this_len
    }

    #[must_use]
    pub fn min_input_len_work(&self) -> usize {
        self.effects
            .iter()
            .filter(|e| e.enabled())
            .map(|e| e.min_input_len_work())
            .max()
            .unwrap_or(0)
    }

    //AI=== 延迟线总时长：格式取管理器登记的输入格式，不信任 info() / delay-line duration: use registered input formats, not info()
    /// 当前效果链内部延迟线总时长 / total internal delay-line duration of the chain
    #[must_use]
    pub fn buffered_duration(&self) -> Duration {
        self.effects
            .iter()
            .enumerate()
            .filter(|(_, effect)| effect.enabled())
            .map(|(index, effect)| {
                self.input_formats[index].samples_to_duration(effect.buffered_samples())
            })
            .sum()
    }
    //AI_END===

    //AI=== 前效果器列表：只读 trait 方法与管理器登记的输入格式 / pre-effect list: read-only trait methods and registered input formats
    /// 前效果器的展示快照 / display snapshot of the pre-effect chain
    #[must_use]
    pub fn effect_summaries(&self) -> Vec<EffectSummary> {
        self.effects
            .iter()
            .enumerate()
            .map(|(index, effect)| EffectSummary {
                chain: EffectChainKind::Pre,
                index,
                name: effect.display_name(),
                description: effect.description(),
                enabled: effect.enabled(),
                can_disable: effect.disable_policy() == DisablePolicy::Allowed,
                latency: if effect.enabled() {
                    self.input_formats[index].samples_to_duration(effect.buffered_samples())
                } else {
                    Duration::ZERO
                },
            })
            .collect()
    }
    //AI_END===

    //AI=== 前效果器启用/禁用：禁用时排空并通知下游，启用时重新初始化 / disabling drains and notifies downstream; enabling reinitializes
    /// 设置指定前效果器的启用状态。
    ///
    /// 禁用时：先让目标排空内部缓冲，再把排出的数据依次交给下游的
    /// `UpstreamDisabled` 处理；随后才把目标置为禁用。启用时：从目标开始
    /// 重新 `reset` 并重算有效格式。
    ///
    /// Set a pre effect's enabled state. Disabling drains the target and feeds the
    /// drained samples through downstream `UpstreamDisabled` handlers before the
    /// target is bypassed. Enabling resets the target and downstream stages and
    /// renegotiates the effective formats.
    pub fn set_effect_enabled(
        &mut self,
        index: usize,
        enabled: bool,
        output: &mut Vec<f32>,
    ) -> Result<(), EffectToggleError> {
        if enabled {
            self.enable_effect(index)
        } else {
            self.disable_effect(index, output)
        }
    }

    fn disable_effect(
        &mut self,
        index: usize,
        output: &mut Vec<f32>,
    ) -> Result<(), EffectToggleError> {
        let Some(effect) = self.effects.get(index) else {
            return Err(EffectToggleError::InvalidIndex);
        };
        if !effect.enabled() {
            output.clear();
            return Ok(());
        }
        // 静态策略是唯一权威：会改变格式的效果器必须返回 Forbidden。
        // The static policy is the only authority: a format-changing effect must return Forbidden.
        if effect.disable_policy() == DisablePolicy::Forbidden {
            return Err(EffectToggleError::Forbidden);
        }

        // 复用链自己的暂存缓冲，避免每次开关都分配两个 Vec。
        // Reuse the chain's scratch buffers so a toggle does not allocate two new Vecs.
        let effects = &mut self.effects;
        let stage_in = &mut self.input_buffer;
        let stage_out = &mut self.output_buffer;

        stage_in.clear();
        let _ = effects[index].on_discontinuity(
            &[],
            &mut *stage_in,
            PreEffectDiscontinuity::DisableSelf,
        );

        for (downstream, effect) in effects.iter_mut().enumerate().skip(index + 1) {
            stage_out.clear();
            if effect.enabled() {
                let _ = effect.on_discontinuity(
                    stage_in.as_slice(),
                    &mut *stage_out,
                    PreEffectDiscontinuity::UpstreamDisabled {
                        distance: downstream - index,
                    },
                );
            } else {
                // 已禁用级没有状态，直接透传排空数据 / a disabled stage has no state, pass the drained data through
                stage_out.extend_from_slice(stage_in.as_slice());
            }
            std::mem::swap(&mut *stage_in, &mut *stage_out);
        }

        effects[index].set_enabled(false);
        output.clear();
        output.extend_from_slice(stage_in.as_slice());
        Ok(())
    }

    fn enable_effect(&mut self, index: usize) -> Result<(), EffectToggleError> {
        let len = self.effects.len();
        let Some(effect) = self.effects.get(index) else {
            return Err(EffectToggleError::InvalidIndex);
        };
        if effect.enabled() {
            return Ok(());
        }

        let mut errors = Vec::new();
        let mut input_format = self.input_formats[index];

        // 先 reset 再 set_enabled：重采样器出错后只有成功的 reset 才能清除错误。
        // Reset before enabling: a failed resampler can only clear its error through a successful reset.
        let target_effective = match self.effects[index].reset(input_format, ResetReason::Toggled) {
            Ok(output) => {
                self.effects[index].set_enabled(true);
                if self.effects[index].enabled() {
                    output.output_format
                } else {
                    input_format
                }
            }
            Err(e) => {
                self.effects[index].set_enabled(false);
                errors.push((self.effects[index].info().clone(), e));
                input_format
            }
        };
        input_format = target_effective;
        self.input_formats[index + 1] = input_format;

        // 目标仍未启用（例如重采样器错误未清除）时，下游信号没有变化，不做额外复位。
        // If the target is still disabled (e.g. a resampler error persists), downstream is unchanged.
        if !self.effects[index].enabled() {
            return if errors.is_empty() {
                Ok(())
            } else {
                Err(EffectToggleError::Reset(errors))
            };
        }

        for downstream in index + 1..len {
            let effective =
                match self.effects[downstream].reset(input_format, ResetReason::InputChanged) {
                    Ok(output) => {
                        if self.effects[downstream].enabled() {
                            output.output_format
                        } else {
                            input_format
                        }
                    }
                    Err(e) => {
                        self.effects[downstream].set_enabled(false);
                        errors.push((self.effects[downstream].info().clone(), e));
                        input_format
                    }
                };
            input_format = effective;
            self.input_formats[downstream + 1] = input_format;
        }

        self.output_format = input_format;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(EffectToggleError::Reset(errors))
        }
    }
    //AI_END===
}

impl Default for PreEffectChain {
    fn default() -> Self {
        Self::new()
    }
}
