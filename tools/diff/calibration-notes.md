## 人工说明：FluidSynth 2.6.1 与 SpessaSynth 4.3.22 的差异

> 这一节是看过校准结果后手写的（`tools/diff/calibration-notes.md`），`calibrate` 会把它原样放进 `out/calibration.md`。数字来自 2026-10-28 的校准，SoundFont 以 GeneralUser GS 2.0.3 为主。值都是 SpessaSynth 减 FluidSynth。用例或引擎版本变了，要重新核对这里的结论。

### 两者一致的地方（NerdSynth 照做即可）

- **CC7、CC11 的曲线**：127 到 16 的每一档，差异都等于整体增益偏移（-1.80 dB），误差在 ±0.1 dB 以内。CC7 或 CC11 为 1 和 0 时，两者都低于 -100 dB。
- **声像的中间段**：CC10 为 16 到 112 时，左右能量比相差不到 0.1 dB。单声道采样（长笛）在 pan 64 时左右相等，立体声采样（钢琴）在 pan 64 时是 +0.55 dB。
- **事件的时机和状态**：bank select 要等下一次 program change 才生效；bank 5 不存在时退回 bank 0；鼓组 program 20 不存在时退回 kit 0；GS 的 use-for-rhythm-part 能把通道 11 变成鼓通道；NRPN 不改变已经选中的 RPN（E-14）；CC64=63 算抬起，64 算踩下；踏板踩着时重复按同一个键；同一 tick 里 program change 与 note on 的先后顺序。这些用例的差异都只有整体增益偏移那么多。
- **exclusive class 只在同一通道内生效**：通道 11 上的闭镲不会切断通道 10 上的开镲（`exclusive/gu/other-channel`）。
- **调制轮和通道触后都会加颤音**，用的是规范的默认调制器，127 时长笛的颤音深度约 39–41 音分。复音触后两边都不起作用。
- **音高**：env 类用例的基频差的中位数是 0.0 音分。

### 差异显著的地方（需要我们做决定）

1. **整体增益**：两边都抵消了自带的主增益（FluidSynth 用 `-g 1`，SpessaSynth 的 gain 设为 1/0.6）之后，SpessaSynth 仍然比 FluidSynth 低 **1.69 dB**（无效果用例的中位数）。这个偏移跟 CC7/CC11 和声像都无关，是主增益的设定不同。NerdSynth 打算对齐 FluidSynth 的电平，`run` 要求偏移在 ±1 dB 以内。
2. **声像的两端，以及实时变化的平滑**：
   - pan 127 时，FluidSynth 的左声道还剩 -44 dB，SpessaSynth 剩 -38 dB（差 6.0 dB）。pan 0 时，FluidSynth 的右声道完全无声，SpessaSynth 有 -56 dB。
   - 发声中改 CC10：FluidSynth 在一个块（64 帧）内跳到新的声像。SpessaSynth 是平滑过去的，从居中到最左，50 ms 时只到 +7 dB，约 400 ms 才到 +56 dB（`pan/gu/live-organ` 在 pan 0 那段的差异是 -37.7 dB）。要决定 NerdSynth 是否平滑，平滑多快。
3. **力度 → 亮度**（decisions.md 待定第一条）：钢琴（`velocity/gu/piano`）在 SpessaSynth 里更亮。质心比在力度 1 时是 1.00，力度 64–96 时是 1.09–1.11，力度 127 时回到 1.02。与此同时，扣掉整体偏移后的相对响度从力度 1–16 的 +1.5 dB 降到力度 127 的约 0 dB。这符合 FluidSynth 那条带力度开关的力度 → 截止频率调制器（SpessaSynth 去掉了这一条）。但力度 64 以下 SpessaSynth 也亮了 0.07–0.10 oct，单用开关解释不了，引擎阶段要逐个声部核对。
4. **与力度无关的亮度差**：锯齿波（`velocity/gu/saw`）在每个力度上，SpessaSynth 都亮 0.16–0.19 oct（质心比约 1.14）。方波、pad、颤音琴、小号、部分鼓也在 +0.08 到 +0.3 oct 之间（`drums/fluid/standard` 的小军鼓是 1.24 倍）。这些预设都带滤波，推测是低通滤波器的实现不同（Q 的换算、截止频率的换算或钳制）。这一条还没有逐项验证。
5. **CC74**（decisions.md 待定第二条）：SpessaSynth 把 CC74 接到截止频率上，FluidSynth 没有。CC74=0 时，锯齿波的质心比是 0.27，响度低 13 dB；弦乐的质心比是 0.56。
6. **弯音范围的 LSB**：RPN 0 设为 1 半音 50 音分时，FluidSynth 的范围正好是 ±1.50 半音。SpessaSynth 的范围约 ±1.39 半音（弯到底差 +12 音分，弯到顶差 -11 音分），像是把 LSB 当成 1/128 半音，而 GM 规定 LSB 的单位是音分。另外，SpessaSynth 向下弯时，音高一直比 FluidSynth 高约 1 音分，推测是负方向按 8191 而不是 8192 归一化，影响很小。
7. **起音的曲线**：起音慢的音色，SpessaSynth 达到峰值下 30 dB 的时刻更晚：`env/gu/strings-60` 晚 144 帧（3.3 ms），`velocity/gu/strings` 力度 1 时晚 146 帧，`env/gu/pad-84` 晚 83 帧。起音快的音色只差 5–15 帧。说明两边起音段的形状不同（FluidSynth 的起音在振幅上是线性的）。
8. **个别预设的包络**：TimGM6mb 的弦乐和方波，在 1.5 s 前后 SpessaSynth 高出 10 dB（`env/tim/strings-60`、`env/tim/square-60`）；TimGM6mb 的弦乐持续音在 4.5 s 时高出 11.5 dB（`loop/tim/strings-60`）；`env/gu/trumpet-36` 在 1.45 s 时低 4 dB；`env/gu/square-84` 在 0.5 s 时高 5 dB。可能是衰减段、延音电平或调制包络对音量的换算不同，要按预设查生成器。
9. **很高的音**：`loop/gu/strings-108` 里，持续段 SpessaSynth 低 2.5 dB，3.6 s 时形状差了 5.4 dB。对应短循环和读取步长大于循环的情形（E-10）。
10. **exclusive class 的停音速度**：两边都会停掉开镲，但 SpessaSynth 停得更快。在 -80 dB 以下，GeneralUser 的踏镲停开镲时，440 ms 处差 -10.6 dB，尾音短 30 ms；TimGM6mb 的尾音短 40 ms。E-06 要求约 2 ms 的淡出，两边都比这慢得多，要决定以谁为准。
11. **复音抢占**：`polyphony/gu/organ-8voices` 前 9 个音完全一致。从第 10 个音开始，两边抢掉的声部不同，包络相关掉到 0.30–0.79。`piano-4voices` 是连续的短音，两边一致（相关 0.998）。抢占的优先级要按 FluidSynth 的 overflow 规则来定。
12. **鼓组**：GeneralUser 的 orchestra kit 里，拍手（39）在 SpessaSynth 里低 2.6 dB（扣掉整体偏移后）；808 kit 的闭镲低 0.9 dB。room、power、brush 这几套 kit 的个别音色，质心差约 +0.2 oct（同第 4 条）。
13. **效果**（fx 组，阈值单独校准）：两边的混响和合唱是完全不同的实现。SpessaSynth 的混响尾音长 290–460 ms，尾部高 7–8 dB；FluidR3 的钢琴在 CC91=127 时，SpessaSynth 的质心只有 FluidSynth 的 0.76 倍（混响更暗）。合唱在 CC93=64 时，左右能量比差 1.8 dB。很轻的音（`fx/gu/quiet-send`）发 send 时，SpessaSynth 整体高 2.5 dB，左右能量比差 7 dB。FluidSynth 这边用的是 `fdn` 混响（2.4 的那种），2.6 默认的 Dattorro 混响没有测。
