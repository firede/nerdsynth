# NerdSynth 设计

> 一个 Rust 写的 SoundFont 2 合成器库：规范完整、按采样精确、实时安全、有公开的差分测试。
> 本文是实现的契约。改动接口或行为时先改这里，再改代码。决定的理由记在 `decisions.md`。

## 目标

- **SoundFont 2.04 完整**：全部生成器（含取值范围）、调制器（含默认调制器和区域层级的合并规则）、采样模式、exclusive class、keynum / velocity 覆盖。链接调制器（目的地是另一个调制器）第二阶段做。SF3（Ogg Vorbis 压缩采样）做成可选特性，第二阶段。
- **MIDI**：通道消息全部支持，含复音触后和通道触后；RPN 0/1/2/5；GM / GM2 / GS 复位；GS 的鼓通道切换和常用 NRPN。XG 第二阶段。
- **按采样精确**：事件带帧时间戳，在那一帧生效，不按块量化。
- **实时安全**：`render` 不分配内存、不加锁、不 panic、不做系统调用。只要输入的长度合法，任意输入都不会让它出现 NaN 或 Inf。
- **行为有出处**：规范写明的照规范；规范没写的照 FluidSynth（2.6，见 decisions.md）；偏离两者的地方记在 `decisions.md`。每条行为在 `behavior.md` 里有编号、出处和对应的测试。
- **有对照**：差分测试台用同样的 MIDI 和 SF2 对比 FluidSynth（主）和 SpessaSynth（辅）的渲染，指标和阈值写在 `tools/diff`。

不做：SFZ、DLS（以后再议）、Web / WASM 专门适配、MIDI 设备输入输出（交给宿主）、音频设备输出（交给宿主）。

## 模块

单个 crate `nerdsynth`，只依赖标准库（可选特性除外）。

| 模块 | 内容 | 依赖 |
| --- | --- | --- |
| `soundfont` | SF2 解析、数据模型、校验；`resolve`：按 preset、key、velocity 解析出要发的声部（生成器叠加、调制器合并） | 无 |
| `event` | `Event`（通道消息、sysex）、从原始 MIDI 字节解析 | 无 |
| `synth` | `Synth`：通道状态、事件队列、声部池、DSP（振荡器、包络、LFO、滤波）、效果（混响、合唱） | `soundfont`、`event` |
| `midi` | SMF 解析、`Sequencer`（循环标记、定位、按帧取事件） | `event` |
| `testkit`（特性 `testkit`） | 在内存里拼 SF2 和 SMF 的构造器，给测试和 fuzz 用 | `soundfont` |

`tools/diff` 是差分测试台，不进 crate。`fuzz/` 是 cargo-fuzz 的目标。

## 公开接口

接口骨架在 `nerdsynth/src`。要点：

- `SoundFont::read(impl Read) -> Result<SoundFont, Error>`，`SoundFont::from_bytes(&[u8])`。加载后不可变，用 `Arc` 在多个 `Synth` 之间共享。
  - 数据模型字段私有，用访问方法读：`info()`、`presets()`、`instruments()`、`samples()`、`sample_data()`、`sample_data24()`；`Info`、`Preset`、`Instrument`、`Sample`、`Modulator`、`Source` 都只有访问方法。
  - `find_preset(bank, program)`，`regions(preset, key, velocity)` 给出 `Region`：`sample()`、`generator(GeneratorType) -> i32`（叠加并钳制后的值）、`modulators(&mut ModList)`（合并后的调制器，不分配）。`ModList` 是不透明的定长表，只公开 `new()`、`as_slice()`、`dropped()`。
  - 不公开：默认调制器表、生成器的个数、范围表和整组生成器的数组、`ModList` 的容量和写入方法。测试和 fuzz 要的构造方法放在 `testkit`（`#[doc(hidden)]`，不属于公开契约）。
- `Synth::new(Arc<SoundFont>, Config) -> Result<Synth, Error>`。`Config` 有 `sample_rate`、`max_voices`、`event_capacity`、`effects`、`interpolation`、`velocity_to_filter`、`cc74_to_filter`、`min_note_length`（默认 10 ms）、`gain`（默认 1），`Config::new(sample_rate)` 给默认值。
- 事件：
  - `synth.send(Event)` 立刻生效，也就是在当前帧。
  - `synth.schedule(frame: u64, Event) -> Result<(), QueueFull>` 在绝对帧 `frame` 生效。帧时钟从 0 开始，每渲染一帧加一。早于当前帧的事件在下次渲染开头生效。同一帧的事件按加入的顺序生效。
  - `synth.now() -> u64` 返回当前帧。
  - 队列容量固定为 `event_capacity`，满了返回 `QueueFull`，不分配内存。
- `synth.render(left: &mut [f32], right: &mut [f32]) -> usize` 渲染 `min(left.len(), right.len())` 帧，返回渲染的帧数。另有 `render_interleaved(&mut [f32])`。
- 查询：`active_voices()`、`is_silent()`（没有发声的声部，效果的尾音也已低于阈值）。
- `Sequencer::new(Arc<Smf>, &Synth)`：用这个 synth 的采样率，记下它此刻的 bank select 方式（GS/GM/GM2）作为起始方式。`Sequencer::with_sample_rate(Arc<Smf>, sample_rate) -> Result<Sequencer, Error>` 给不经过 `Synth` 的宿主，采样率范围同 `Config`（8000–192000），起始方式是 GS（同新建的 `Synth`）。
  - `next_events(until_frame, |frame, event|)` 按帧交出事件，测试和其他宿主可以不经过 `Synth` 直接用。
  - 便捷方法 `render(&mut Synth, left, right)`，按 synth 的采样率播放：采样率不同时，从已播到的时间起改按 synth 的采样率换算。
  - 另有 `position()`、`seek(Duration)`（追溯状态；会分配内存，耗时与目标之前的事件数成正比，不要在音频线程上调用）、`set_looping(bool)`、`finished()`。
  - 循环点有两种：SMF 里的循环标记（`LoopMarkers`：RPG Maker 的 CC111、FF 的 CC116/117 等）和整曲循环。
  - 同一 tick 的循环标记排在其他事件之前；没有起点标记时，回到时间 0。
  - 循环区间是半开的 [S, E)：S 是第一个起点标记，没有就是 0；E 是 S 之后的第一个终点标记，没有就是曲末。短于 1 ms 的循环不播，见 decisions.md。
  - 回卷和 `seek` 都是跳转。跳转只作用于文件用到的通道（解析时记下：有通道消息或 GS rhythm part 的通道），依次：
    1. 交出这些通道的 CC64=0、CC66=0、CC123、CC121，释放它们的音；
    2. 复位这些通道：`render` 在这一帧让 synth 的这些通道回到初始状态（同 `Synth::reset` 对通道的处理，但不停声部、不清效果），bank select 方式（全局的）取目标处（含目标那个 tick 上的）文件里最后一条 GM/GM2/GS 复位消息设的，没有就取起始方式。这是 crate 内部的队列项（带通道掩码），公开的 `Event` 没有对应的变体；`next_events` 只交出那条复位消息（如果有）；
    3. 交出目标位置的状态：其他 sysex，然后每个通道的结构性事件（bank select、program、RPN/NRPN 的选择和数据、CC121、GS rhythm part）按原来的顺序，去掉效果在被读到之前就被覆盖的；最后是与结构性事件留下的状态不同的 CC、弯音、通道触后、复音触后。

    这样后面的内容在 synth 上像从头播到这里一样，不论跳转之前播到哪里、改过什么。追溯的判定（CC121 重置什么、哪些 sysex 有效）与 `Synth` 相同。文件没用到的通道留给宿主，跳转不碰它们（bank select 方式除外）。
  - 目标那个 tick 上有 GM/GM2/GS 复位消息时，从其中最后一条之后接着播：它由第 2 步的内部复位代替（方式取它的），同 tick 上它之前的事件被它撤销，不再交出；它之后的照常。所以循环起点（常见的是 tick 0）的复位不会每圈都停掉所有声部、清空效果的延迟线。从头播时这条消息照常执行，作用于所有通道。
  - `finished()`：所有事件都已交出，播到了曲末，并且不循环。
  - 事件时间用精确的整数单位保存（1/d 秒），换算到帧时四舍五入；每圈循环加上精确的长度，所以多少圈都不累积误差。
- `Error` 是 `#[non_exhaustive]` 枚举，实现 `std::error::Error`，在公开模块 `error` 里（根上也导出）：`Io(io::Error)`；`Invalid(InvalidFile)`，`InvalidFile::format()` 说是 SoundFont 还是 MIDI 文件，说明文字给人看、不是接口；`Unsupported(Unsupported)`，`CompressedSamples`（SF3）、`MidiFormat2`；`Config(Setting)`，指出哪个设置越界（`SampleRate`、`MaxVoices`、`EventCapacity`、`MinNoteLength`、`Gain`）。`Format`、`Unsupported`、`Setting` 都是 `#[non_exhaustive]`。
- 兼容性：
  - 以后可能加变体的公开枚举都是 `#[non_exhaustive]`：`Event`、`Error`、`error::Format`、`error::Unsupported`、`error::Setting`、`Interpolation`、`VelocityToFilter`、`LoopMarkers`、`SampleKind`。`GeneratorType` 不是：它就是规范的 0–60 号。
  - `Config` 是 `#[non_exhaustive]` 结构体，用 `Config::new` 再改字段；其他公开结构体的字段都私有。
  - `SysEx::CAPACITY`（32）公开，以后只升不降。
- 公开类型都实现 `Debug`。`SoundFont`、`Smf`、`Sequencer`、`Synth` 的是手写的摘要（数量、时长、设置、时钟），不打出采样和事件。

## 引擎

- **控制速率**：每个声部按 64 帧一段更新包络、LFO、调制器和滤波器的目标截止频率，增益在段内线性过渡。段按声部各自维护，从声部开始时算起；事件只切开它改动的声部（同一通道的声部、被释音或淡出的声部）的段，所以事件在准确的帧生效，其他声部不受影响，输出也不随 `render` 的调用长度变化。
- **成本**：一个事件的控制成本只与它影响的声部数有关；但每个事件都会把渲染切成更短的块，每块要遍历所有声部。一次 NoteOn 最多起 `max_voices` 个声部；采样没东西可播的区域加载时就排除；每个区最多 64 条调制器。一次 NoteOn 的耗时为 O(V log V)（V 为声部槽数），只抢一个声部时为 O(V)，加上遍历 preset 的区域对（受 `MAX_REGIONS` 约束）。
- **工作预算**：事件的工作从一个按渲染帧数补充的预算里扣，单位约为本机 1 ns：每秒补 2.5 亿（约占实时的四分之一），最多存 200 万（约 2 ms，默认复音下够一次起 256 个音）。代价按事件估：每个事件 64；扫一遍声部槽的（NoteOff、踏板抬起、All Notes/Sound Off、复位）加槽数；NoteOn 另加 4×槽数、4×区域对数、每个起的声部 2048 加 32×调制器数；被事件切开段、被 exclusive class 截停的声部各 256 加 8×调制器数；事件切出的块 64 加 4×槽数；复位 4096，清效果器延迟线各 16384。预算用完（≤ 0）时，到期的事件留在队列里按顺序等，只在帧号为 64 的倍数、预算回到正数时再生效：为等预算切出的块本身要扣费（遍历声部槽），高采样率或大复音时比等一帧挣的还多，会让吞吐塌缩。为了让何时生效不随 `render` 的调用长度变化：都是每帧的整数；render 预测某个未来事件到时预算仍不够、因而越过它时，也记为「在等」；切段的代价攒到下一个 64 帧的倍数再扣，网格之间的预算只随事件和帧数变化。`send` 在预算用完或有事件在等时也进队列；队列满时只强制处理最老的一个到期事件腾位置。`Sequencer::render` 只把队列排到剩八分之一，留给宿主自己的事件；队列满且 synth 不收时，渲染到 synth 能收的那个网格帧或宿主排的下一个事件，不逐帧走。于是单次 render 或 send 的工作至多为：预算余额（≤ 2 ms）加上本次帧数对应的四分之一，加上一个事件（最贵的是 NoteOn：遍历区域对加起 `max_voices` 个声部）。NoteOn 的代价随槽数（`max_voices` 的两倍）增长，4096 复音时一次突发只够约 60 个音。
- **生成器**：`resolve` 给出每个声部叠加后的生成器（instrument 的值加上 preset 的偏移），按规范的范围钳制。调制器的输出在发声时加上去，再钳制一次。initialAttenuation 的生成器值乘 0.4（同 FluidSynth、SpessaSynth，见 decisions.md）。
- **调制器**：默认调制器 + instrument 全局区 + instrument 区（相同的调制器替换），preset 全局区 + preset 区（相同的替换）后**加到** instrument 级的结果上（相同的相加，不同的追加）。来源取值、极性、方向、曲线按规范；`NoController` 取 1。只在通道状态变化（版本号）或复音触后变化时重新求值。
- **振荡器**：默认四点三次插值，可选线性；循环模式 0/1/3；读位置用 64 位定点；任何采样头和偏移都钳制在数据内。
- **音量包络**：延迟、起音（振幅线性）、保持、衰减和释音（分贝线性），同 FluidSynth。各段长度照 FluidSynth 取整到 64 帧的块，包络在段内逐帧推进；计数用 u64。
- **调制包络、LFO**：同规范；LFO 是三角波，带延迟。
- **滤波**：二阶低通，Q 按 cB 计，同 FluidSynth 2.6：截止频率按音分逐帧滑向目标（64 帧乘 Q，Q 取 1 到 5），Q 在 64 帧内滑向目标，滑动期间每帧查表重算系数；截止频率钳制在 0.45 倍采样率以下。状态超出共振能达到的范围时停掉该声部。
- **声部分配**：满了按优先级抢占（释音中的、更安静的、更老的先被抢）。被抢的声部和 All Sound Off 用约 2 ms 快速淡出，不硬切；被 exclusive class 停掉的声部照 FluidSynth 用 -2000 tc（约 315 ms）的快速释音。exclusive class 只在同一通道、同一 preset 内生效。
- **效果**：混响和合唱各一条总线，声部按 reverb / chorus send 送入。混响是 8 条线的 Jot 反馈延迟网络（线长 46–188 ms，Walsh–Hadamard 反馈，13 ms 预延迟和 4 个全通作输入扩散，一阶搁架吸收滤波按 DC、8 kHz 和顶端三点定衰减时间，0.3 ms 的慢速调制用一阶全通插值读），合唱是三声部正弦调制的延迟线（0.2 Hz，摆动 4.25 ms，中心 3.5 ms，三次插值，左右不反相）。都是独立实现（见 decisions.md「代码来源与许可」），参数固定。质量用 `tools/fxmetrics` 的客观指标对着 FluidSynth 判定（见 decisions.md「效果器的质量目标」）。静音时只推进调制，不处理延迟线。
- **数值安全**：所有计数用 u64；所有来自文件和消息的数值进入 DSP 前都钳制过；效果的状态带非有限值保护。

## 测试

- **单元测试**：每条 `behavior.md` 的行为都有测试，用 `testkit` 拼最小 SF2，断言精确的结果，比如包络到达的时间、某帧的电平、音高的频率、循环点。
- **效果器指标**：`tools/fxmetrics`，`mise run fx-metrics`。同样的测试信号（单击、冲击串、噪声、96 个正弦）分别让 FluidSynth（黑盒）和 NerdSynth 渲染，湿信号取开减关，算混合时间、回声密度、尾音频谱粗糙度、各音尾音电平离散度、衰减平滑度、IACC、合唱带外 SNR 等质量指标和 RT60、起音、电平、重心等性格指标，报告写到 `tools/fxmetrics/out/`。定义和容差见它的 README。改动效果器时要跑。
- **对抗探针**：`nerdsynth/tests/adversarial.rs`，全部 `#[ignore]`，`mise run adversarial` 在 release 下跑。来自五轮独立审查的探针：恶意 SF2 的加载和 NoteOn、各采样率和复音数下的事件洪泛、Sequencer 的密集事件和极短循环、宿主与 Sequencer 共用队列、预算吃紧时 render 长度无关（逐位比较）、随机 Sequencer 压力测试；`cost_report` 打印当前机器上各类工作的耗时，用来对照 `synth/budget.rs` 的代价单位。计时的限取「跟得上实时」并留几倍余量（开发机实测值见 decisions.md），debug 下只检查不计时的部分。改动预算、声部分配、队列、Sequencer 或解析时要跑。
- **差分测试**：`tools/diff` 生成一组针对单项行为的 MIDI，例如单音、力度扫描、CC 扫描、弯音、循环、鼓组、exclusive class，用 FluidSynth、SpessaSynth 和 NerdSynth 各渲染一遍，计算这些指标：
  - 包络相关
  - 响度差（dB）
  - 基频误差（音分）
  - 频谱质心比
  - 起音时间差
  
  阈值先用 FluidSynth 与 SpessaSynth 之间的差异校准。
- **Fuzz**：SF2、SMF 的解析，以及"解析加上发几个音再渲染"，都不得 panic，不得出现非有限值，单次操作的耗时和分配有上界。
- **基准**：`render` 的吞吐量（声部 × 实时倍数），以及加载时间和峰值内存。
