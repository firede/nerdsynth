# 差分测试台（`nerdsynth-diff`）

用同样的 MIDI 和 SF2，分别让 FluidSynth（主参照）、SpessaSynth（辅参照）和 NerdSynth 渲染，逐段计算指标，再按阈值判定。指标要能指出"哪里不一样"：哪个用例、哪一段、哪个指标、差多少，都写在报告里。

阈值先拿 FluidSynth 对 SpessaSynth 校准：两个成熟实现之间的差异，就是"自然差异"的尺度。两者差得远的用例，正是规范没写死、需要我们做决定的地方，见 `calibration-notes.md`。

## 安装

```sh
brew install fluid-synth   # 开发依赖；校准时用的是 2.6.1
mise run diff:setup        # 在 tools/diff/spessa 里 pnpm install（spessasynth_core 4.3.22）
```

另需 node 24，以及下面三个 SoundFont。工具按顺序在 `$NERDSYNTH_SOUNDFONTS`、`~/.nerdsynth/soundfonts`、`~/.nerdice/soundfonts` 里找。

| 文件 | 来源 | 许可 |
| --- | --- | --- |
| `GeneralUser-GS-2.0.3.sf2` | nerdice 已经下载到 `~/.nerdice/soundfonts`（S. Christian Collins） | 作者自己的 GeneralUser GS 许可，这次没有重新核对 |
| `FluidR3_GM.sf2` | Debian `fluid-soundfont_3.1.orig.tar.gz`（http://deb.debian.org/debian/pool/main/f/fluid-soundfont/），sha256 `74594e8f…bc1cb0` | MIT（Frank Wen，见包内 COPYING） |
| `TimGM6mb.sf2` | Debian `timgm6mb-soundfont_1.3.orig.tar.gz`（http://deb.debian.org/debian/pool/main/t/timgm6mb-soundfont/），sha256 `c5378b62…c854` | GPL-2（Tim Brechbill，见 Debian 的 copyright 文件） |

SoundFont 不进仓库。

## 用法

```sh
mise run diff:refs         # 用两个参照渲染全部用例；已渲染、版本没变的跳过
mise run diff:calibrate    # 比较 SpessaSynth 和 FluidSynth，重写 thresholds.json 和 out/calibration.md
mise run diff              # NerdSynth 对 FluidSynth（引擎就绪之后）

cargo run --release -p nerdsynth-diff -- list                        # 列出用例
cargo run --release -p nerdsynth-diff -- run --synth spessasynth,fluidsynth --filter env/gu
cargo run --release -p nerdsynth-diff --features nerdsynth -- run --synth nerdsynth,fluidsynth,spessasynth
```

- `--filter A,B`：只跑 id 含 A 或 B 的用例。过滤后的 `calibrate` 不写 `thresholds.json`。
- `--force`：重新渲染。NerdSynth 每次都会重新渲染。
- `--ref`：`run` 的参照，默认 `fluidsynth`。`--synth` 里其余的每个都和参照比一次，报告写到 `out/report-<对象>-vs-<参照>.md`，逐段的数值写到 `out/results/*.json`。有用例超出阈值，或者整体增益偏移超过 ±1 dB 时，退出码是 1。
- 全部用例渲染加比较约 12 秒（10 核）。

`out/` 被 git 忽略，里面有：`cases/`（每个用例的 `.mid`，以及带帧号的事件 `.json`）、`render/<synth>/`（WAV 和 `VERSION`）、报告和结果。`thresholds.json` 进仓库。

### NerdSynth 的接入点

`src/render.rs` 里的 `nerdsynth()` 已经写好：加载 SF2，用 `Config` 设好采样率、复音数、effects 和队列容量，按帧 `schedule` 每个事件，再分块 `render`。它在 cargo 特性 `nerdsynth` 后面，默认不编译进来（`Synth` 还是 `todo!()`），不开这个特性时要求渲染 NerdSynth 会直接报错。引擎就绪以后，`mise run diff` 一条命令就能加进比较；也可以把这个特性设成默认开启。

## 渲染：怎么做到可比

- **统一格式**：44100 Hz，立体声，32 位浮点 WAV。渲染完一律截取或补零到用例的长度，再用工具自己的格式重写一遍。
- **对齐起点，事件对到同一帧**：MIDI 是 500 tick/四分音符、120 bpm，所以 1 tick = 1 ms。FluidSynth 的播放器只在每个 64 帧块的开头运行，那个块的时间是 `msec = (long)(1000·64k/rate)`，它发出 tick 不超过 `msec` 的事件。事件对声部的作用（发音、控制器）要排队，等下一次渲染调用开头才执行，也就是晚一个块。所以一个时间为 t 的事件，在"第一个 msec ≥ t 的块"的下一块生效（`midi::fluid_frame`，已用渲染结果核对过）。SpessaSynth 和 NerdSynth 拿到的是同样的帧号：SpessaSynth 渲染到那一帧就停下来送事件，NerdSynth 用 `schedule(frame, …)`。这样三者在同一帧收到同一个事件，起音差测的就是包络本身。
- **FluidSynth 的命令行**（`render::fluidsynth_args`）：

| 参数 | 原因 |
| --- | --- |
| `-F out.wav -O float -T wav -E little` | 快速离线渲染成 32 位浮点 WAV，不截幅、不抖动 |
| `-r 44100` | 统一采样率 |
| `-g 1` | 主增益设为 1。默认的 0.2 只是缩放，会干扰响度比较 |
| `-R 0 -C 0`（fx 用例为 `1 1`） | 默认开着混响和合唱；无效果用例关掉，有效果的用例单独标为 fx |
| `-o synth.reverb.engine=fdn` | 2.5 起默认换成了 Dattorro 混响；fdn 是 2.4 的混响，decisions.md 的参照是 2.4 |
| `-o synth.polyphony=N` | 默认 256；复音用例改成 8 或 4 |
| `-o audio.period-size=64` | 每次渲染调用只渲染一个块，事件才会准确地晚一个块生效。更大的周期会把事件拖到周期边界 |
| `-o synth.midi-bank-select=gs`、`synth.cpu-cores=1`、`synth.dynamic-sample-loading=0`、`player.reset-synth=1` | 都是默认值，写明是为了固定下来：GS 的 bank select、单线程渲染（结果确定）、采样一次加载完、播放前复位 |
| `-q -n -i` | 不打印、不开 MIDI 输入、不开 shell |

  没有改的默认值：插值是四点插值（FluidSynth 的默认）；`synth.min-note-length=10`（用例里没有短于 10 ms 的音）；overflow（抢占）的权重；`synth.device-id=16`（用例里的 GS sysex 用 0x10）。文件里的事件放完后，播放器会抬起踏板、关掉所有音，再等声部结束；用例的 End of Track 放在用例结束的时刻，之后的输出截掉。
- **SpessaSynth**（`spessa/render.mjs`）：`SpessaSynthProcessor`，`gain = 1/SPESSASYNTH_GAIN_FACTOR`（抵消它自带的 0.6），`voiceCap` 等于用例的复音数，hermite 插值（它的默认），`effectsEnabled` 只在 fx 用例打开。每个进程只加载一次 SoundFont，几个进程并行。
- **NerdSynth**：见上一节。

## 用例

用例是 `src/cases.rs` 里的代码表。一个用例包括：MIDI 事件（程序生成）、用哪个 SoundFont、复音数、是否 fx、测量的分段（每段可以带期望音高和测音高的时间窗），以及判定用的指标（focus）；其余指标只报告，不判定。id 的格式是 `类别/sf2/名字`，要保持稳定。共 151 个：

| 类别 | 测什么 | 数量 |
| --- | --- | ---: |
| `env` | 12 个 GM program（钢琴、电钢、颤音琴、风琴、吉他、贝斯、小提琴、弦乐、小号、长笛、方波、pad），各发一个音，按住 1.5 s：包络、音高、亮度、尾音。GeneralUser 用 key 36/60/84，另两个只用 60 | 60 |
| `velocity` | 同一个音，力度 1–127 九档：衰减曲线和亮度（力度 → 截止频率） | 6 |
| `volume` | 按住一个音，CC7 或 CC11 分九档变化，以及两者都设为 64 | 5 |
| `pan` | 每个音前设一个 CC10 值（九档），以及按住时改变 CC10 | 5 |
| `bend` | 默认弯音范围；RPN 0 设为 12、1.50、24；NRPN 不改变 RPN | 8 |
| `vibrato` | 调制轮、通道触后、复音触后，各三档 | 9 |
| `cc74` | 按住时改变 CC74 | 2 |
| `loop` | 持续 4 s 的循环采样，包括 key 108（短循环） | 14 |
| `pedal` | hold pedal：踩着时松开音、再按同一个键、CC64 取 63 和 64 | 6 |
| `drums` | 8 套 kit 各 10 个鼓件；不存在的 kit 退回；GS rhythm part 的 sysex | 14 |
| `exclusive` | 开镲被闭镲或踏镲停掉，同一个键重复，跨通道不停 | 8 |
| `polyphony` | 8 个声部里加和弦；4 个声部里快速弹短音 | 2 |
| `program` | 同一 tick 里 program change 的先后、发声中换 program、bank select 要等 program change、不存在的 bank 退回 | 7 |
| `fx` | 混响 send（0/40/127）、合唱 send、很轻的音的 send、默认 send（效果开着，单独计阈值） | 5 |

加用例：在 `cases::all()` 调用的某个函数里加一个 `case(...)`，或者新写一个函数再加进 `all()`；然后 `mise run diff:calibrate`。`cargo test -p nerdsynth-diff` 会检查 id 不重复、分段不越界。

## 指标

先把每个渲染算成 10 ms 一帧的功率包络（两个声道平方的均值，以 dB 计）。在每一段里，比较对象和参照。用例的值取最差的那一段，报告里写出是哪一段，附上细节，例如"在 2060 ms 处，参照 -84.1 dB，对象 -84.3 dB"。

| 指标 | 定义 | 偏差（拿来和阈值比的数） |
| --- | --- | --- |
| `env_corr` | 两个 dB 包络的皮尔逊相关。先把两者都钳在较响那个峰值下 60 dB 处，只算至少有一方高于这条线的帧；两条都是平的就算 1 | 1 − 相关 |
| `env_dev_db` | 两个包络之差，减去差的中位数之后的最大绝对值，也就是形状最大偏离的地方，带上时刻 | 绝对值 |
| `loudness_db` | 一段的能量比，以 dB 计 | 和本次运行整体增益偏移的距离。整体增益偏移是无效果用例整段响度差的中位数；主增益不同只判一次，限值 ±1 dB |
| `f0_cents` | 基频中位数之差，以音分计。1024 帧一窗、跳 256 帧，求归一化平方差最小的延迟；只在期望周期上下 15 个半音的范围里找（有的预设本来就差一个八度，例如 GeneralUser 的风琴是 16'），取最短的、比最小值只多 0.05 以内的局部极小值，再做抛物线插值 | 绝对值 |
| `vibrato_cents` | 颤音深度之差：音高轨迹的（p90 − p10）/ 2 | 绝对值 |
| `centroid_oct` | 频谱质心之比的 log2，以 oct 计。2048 点 Hann 窗、跳 512，按每帧能量加权，只算峰值下 40 dB 以内的帧。细节里给出比值 | 绝对值 |
| `bands_db` | 五个频段（<250、250–1k、1k–4k、4k–16k、>16k Hz）各占总能量的份额（dB），取差得最多的那个频段 | 绝对值 |
| `onset_frames` | 各自第一次达到本段峰值下 30 dB 的帧之差 | 绝对值 |
| `balance_db` | 左右能量比之差，以 dB 计（每边钳在 ±60） | 绝对值 |
| `tail_ms` | 各自最后一次高于本段峰值下 60 dB 的时刻之差 | 绝对值 |

两边都低于 -100 dB 的段跳过；只有一边是静音的段只给出响度（会是很大的数，判为不通过）。

## 阈值和它们的来源

`thresholds.json` 由 `calibrate` 生成：对每个指标、每组（dry / fx），把以这个指标判定的用例的偏差收集起来，阈值 = max(下限, 3 × 中位数)，再向上取两位有效数字。中位数代表两个实现之间典型的差异；远远超出它的用例，就是行为上的分歧，不该算进"自然差异"，所以用中位数而不用最大值或高分位。下限（`judge::floor`）是指标分辨不了、或者人耳听不出的量：

| 指标 | 下限 | dry 阈值 | fx 阈值 |
| --- | ---: | ---: | ---: |
| env_corr（1 − r） | 0.002 | 0.002 | 0.026 |
| env_dev_db | 1.5 | 3.9 | 25 |
| loudness_db | 0.5 | 0.5 | 4.2 |
| f0_cents | 2 | 2 | – |
| vibrato_cents | 3 | 3 | – |
| centroid_oct | 0.05 | 0.065 | 0.37 |
| bands_db | 1 | 3 | – |
| onset_frames | 64 | 64 | – |
| balance_db | 0.5 | 0.65 | 3.7 |
| tail_ms | 30 | 30 | 990 |

（上表是 2026-10-28 的校准结果，以 `thresholds.json` 为准。）整体增益偏移的限值 ±1 dB 不是校准出来的：NerdSynth 打算对齐 FluidSynth 在增益 1 时的电平。

注意：
- 这些阈值是"两个不同实现之间"的尺度。NerdSynth 照 FluidSynth 来做，对 FluidSynth 理应更接近，所以这些阈值是宽松的上界。
- 效果例外：NerdSynth 的混响（八条线的反馈延迟网络）和合唱是独立实现，不是 FluidSynth 的移植，只把电平、衰减时间和延迟的量级调得靠近 FluidSynth（`tools/fxmetrics` 逐项对比）。fx 组和 SpessaSynth 一样，是"两个不同实现之间"的比较。
- 在校准里超出阈值的用例（74 / 151），换成 NerdSynth 对 FluidSynth 时同样要判。对这些地方，NerdSynth 要么照 FluidSynth 来，要么在 decisions.md 里记下偏离，再给这个用例单独放宽。单独放宽的机制还没有做。
- fx 组只有 5 个用例，阈值很粗。
