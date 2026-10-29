# NerdSynth

[English](README.md) | 简体中文

[![CI](https://github.com/firede/nerdsynth/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/firede/nerdsynth/actions/workflows/ci.yml)
[![docs.rs](https://img.shields.io/docsrs/nerdsynth)](https://docs.rs/nerdsynth)

NerdSynth 是一个用 Rust 写的 SoundFont 2 合成器：规范完整、按采样精确、实时安全。除标准库外没有依赖。

- **SoundFont 2.04**：全部生成器（含取值范围）、调制器（含默认调制器，按规范在各层区域间合并）、采样模式、exclusive class、keynum 和 velocity 覆盖、24 位采样。
- **MIDI**：全部通道消息，含复音触后和通道触后；RPN 0、1、2、5；GM、GM2、GS 复位；GS 鼓通道和常用 NRPN。还有一个标准 MIDI 文件播放器：支持循环（整首循环，或 CC111 等循环标记）、定位，也可以按帧把事件交给任意宿主。
- **按采样精确**：事件带帧时间，在那一帧生效，而不是等到下一个块。
- **实时安全**：渲染从不分配内存、加锁或 panic，任何输入都不会让它输出 NaN 或无穷大。工作预算防止大量涌入的事件拖慢渲染：事件会顺延，而不是卡住音频。
- **混响和合唱**两路发送效果。

> [!WARNING]
> NerdSynth 是一个早期的个人项目，0.x 版本之间 API 随时可能变化。

规范没有写明的地方，NerdSynth 的行为与 FluidSynth 2.6 一致。差分测试台（`tools/diff`）用两者渲染同样的 MIDI 并比较结果。偏离规范或 FluidSynth 的地方，理由都记在 [`spec/decisions.md`](spec/decisions.md)。

## 使用

```toml
[dependencies]
nerdsynth = "0.1"
```

弹一个音：

```rust,no_run
use std::sync::Arc;
use nerdsynth::{Config, Event, SoundFont, Synth};

let sound_font = Arc::new(SoundFont::read(std::fs::File::open("piano.sf2")?)?);
let mut synth = Synth::new(sound_font, Config::new(44100))?;
synth.send(Event::NoteOn { channel: 0, key: 60, velocity: 100 });
// 半秒后松开。
synth.schedule(22050, Event::NoteOff { channel: 0, key: 60 }).unwrap();

let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
synth.render(&mut left, &mut right);
```

播放 MIDI 文件：

```rust,no_run
use std::sync::Arc;
use nerdsynth::midi::LoopMarkers;
use nerdsynth::{Config, Sequencer, Smf, SoundFont, Synth};

let sound_font = Arc::new(SoundFont::read(std::fs::File::open("gm.sf2")?)?);
let smf = Arc::new(Smf::read(std::fs::File::open("song.mid")?, LoopMarkers::None)?);
let mut synth = Synth::new(sound_font, Config::new(48000))?;
let mut sequencer = Sequencer::new(smf, &synth);

let (mut left, mut right) = (vec![0.0; 512], vec![0.0; 512]);
while !sequencer.finished() || !synth.is_silent() {
    sequencer.render(&mut synth, &mut left, &mut right);
    // 把 `left` 和 `right` 交给音频设备。
}
```

加载文件、创建合成器和播放器都会分配内存：请在音频线程之外完成，再移交给音频线程。`SoundFont` 加载后不可变，可以由任意多个合成器共享。

NerdSynth 只负责渲染音频，不直接对接音频或 MIDI 设备。输出可以搭配 `cpal` 之类的 crate，输入可以搭配 `midir`。

## 尚不支持

- SoundFont 3（Ogg Vorbis 压缩采样），以及链接调制器（目标是另一个调制器的调制器）：前者的文件会被拒绝，后者的链接会被忽略。
- XG，以及 format 2 的 MIDI 文件。
- SFZ 和 DLS。

## 健壮性

文件被当作不可信的输入：格式有误的 SoundFont 和 MIDI 文件要么报错，要么在说得通的范围内播放，绝不会 panic，所有解析器都经过 fuzz。工作预算是尽力而为的保护：病态的文件或事件流会被顺延，而不会卡住音频，但不承诺单次操作耗时的严格上界。

## 文档

API 文档在 [docs.rs](https://docs.rs/nerdsynth)（英文）。NerdSynth 的构建方式、每条行为的出处和测试，以及偏离规范或 FluidSynth 的理由，都在 [`spec/`](spec)：[设计](spec/design.md)、[行为](spec/behavior.md)和[决定](spec/decisions.md)。

## 开发

任务定义在 [`mise.toml`](mise.toml)：

```sh
mise run check        # 测试、rustfmt 和 clippy
mise run msrv         # 用 Rust 1.85 编译
mise run adversarial  # 针对恶意输入的计时探针，release 模式
mise run diff         # 用 NerdSynth 和 FluidSynth 渲染全部用例并比较
```

部分测试要用 GeneralUser GS 2.0.3（`$NERDSYNTH_GU_SF2` 或 `~/.nerdice/soundfonts/GeneralUser-GS-2.0.3.sf2`），找不到时跳过。差分测试台要用 GeneralUser GS 和 FluidR3 GM（在 `$NERDSYNTH_SOUNDFONTS`、`~/.nerdsynth/soundfonts` 或 `~/.nerdice/soundfonts` 里找），并需要安装 FluidSynth（`brew install fluid-synth`），详见 [`tools/diff`](tools/diff/README.md)。fuzz 目标在 [`fuzz/`](fuzz)（`cargo +nightly fuzz run synth_play`）。

## 最低支持的 Rust 版本

1.85。提高最低版本不算破坏性变更，但不会轻易提高。

## 许可

可任选以下两种许可之一：

- Apache License 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT 许可（[LICENSE-MIT](LICENSE-MIT)）

除非你明确声明，否则你有意提交、希望纳入本项目的贡献（按 Apache-2.0 许可的定义），都按上述双许可授权，不附加任何其他条款或条件。
