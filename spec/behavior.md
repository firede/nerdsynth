# NerdSynth 行为清单

> 每条行为一个编号，写明出处（规范章节 / FluidSynth / GS / 来自 go-meltysynth 的问题）和测试位置。实现一条，就补上测试位置。
> 出处缩写：SF = SoundFont 2.04 规范，FS = FluidSynth 2.6，GS = Roland GS，GM = go-meltysynth 的审查或 fuzz 发现。

## 解析：SF2（`soundfont`）

| 编号 | 行为 | 出处 | 测试 |
| --- | --- | --- | --- |
| P-SF-01 | RIFF 块按声明的大小读取，奇数大小补齐一字节；未知块跳过；块大小超出所在 LIST 的报错 | SF §5, GM | `soundfont::tests::unknown_and_odd_chunks_are_skipped`、`a_chunk_larger_than_its_list_is_an_error`、`every_truncation_fails_cleanly` |
| P-SF-02 | 短读（reader 一次只给一部分）不影响结果 | GM | `soundfont::tests::short_reads_do_not_matter`、`read_errors_are_io_errors` |
| P-SF-03 | 声明的数量很大而数据很少时，不按声明的数量预分配 | GM fuzz | `tests/memory.rs::huge_claims_allocate_little`、`soundfont::tests::huge_claims_in_a_small_file_fail_quickly` |
| P-SF-04 | phdr/inst/shdr 的终止记录必须有，按它界定最后一项的区域 | SF §7 | `soundfont::tests::header_lists_need_their_terminal_record`、`the_terminal_record_ends_the_last_item` |
| P-SF-05 | 区域、生成器、调制器的下标越界或倒序时报错，不 panic | GM | `soundfont::tests::indices_out_of_range_or_order_are_errors` |
| P-SF-06 | 第一个 sampleID（preset 区是 instrument）生成器结束一个区，其后的生成器忽略；没有它的区：第一个是全局区，其余的忽略 | SF §7.3, §7.7 | `soundfont::tests::global_zones_and_zones_that_play_nothing`、`generators_after_the_link_are_ignored` |
| P-SF-07 | 每个区（含全局区）里，keyRange 必须是第一个生成器，velRange 只能跟在 keyRange 后面；不符合时忽略这个生成器 | SF §8.1.2 | `soundfont::tests::key_and_velocity_ranges_count_only_in_their_places`、`ranges_in_the_global_zone_apply_to_the_others` |
| P-SF-08 | 同一个区里重复的生成器，后一个有效 | SF §8.1.2 | `soundfont::tests::of_two_identical_generators_the_later_counts` |
| P-SF-09 | SF3 和其他压缩采样报"不支持"，不当作 PCM 读 | GM | `soundfont::tests::soundfont_3_is_unsupported` |
| P-SF-10 | sm24 块存在且大小匹配时用 24 位采样，否则忽略它 | SF §6.2 | `soundfont::tests::sm24_counts_when_valid` |
| P-SF-11 | 采样头的位置越界、循环在采样外的，加载时不报错；发声时钳制，循环无效时当作不循环 | GM | `soundfont::tests::sample_headers_outside_the_data_load`（加载）、`synth::tests::p_sf_11_headers_outside_the_data_are_clamped`（发声） |
| P-SF-12 | 采样数据按块读取，峰值内存不超过数据大小的约 1.2 倍 | GM M6 | `tests/memory.rs::loading_allocates_little_more_than_the_samples`、`tests/generaluser.rs::generaluser_gs` |
| P-SF-13 | 区域总数（preset 区 × instrument 区）有上界，避免一个音拉起几百万个声部；大全局区的调制器不按区复制，加载时间与文件大小成线性 | GM M1 | `soundfont::tests::regions_are_bounded`、`merged_modulators_are_bounded`、`big_global_zones_load_in_linear_time` |
| P-SF-14 | originalPitch 为 255 时当作 60 | SF §7.10 | `soundfont::tests::unpitched_samples_play_at_60` |
| P-SF-15 | ROM 采样（sampleType 的 0x8000 位）忽略 | SF §7.10 | `soundfont::tests::rom_samples_are_left_out` |

## 解析：SMF（`midi`）

| 编号 | 行为 | 出处 | 测试 |
| --- | --- | --- | --- |
| P-MF-01 | 格式 0/1 合并轨道；同一 tick 的事件按轨道顺序排，循环标记排在最前 | GM H1 | `midi/tests.rs`: `same_tick_keeps_track_order_then_track_order`、`loop_start_marker_plays_notes_of_its_tick_in_earlier_tracks`、`loop_end_marker_goes_before_notes_of_its_tick` |
| P-MF-02 | running status 跨过 meta 和 sysex 后仍然有效 | GM | `midi/tests.rs`: `running_status_goes_on_after_meta_and_sysex` |
| P-MF-03 | 缺少 End of Track 的轨道到块尾结束；EOT 之后的事件忽略 | GM | `midi/tests.rs`: `track_without_end_ends_with_its_chunk`、`events_after_end_of_track_are_ignored`、`end_of_track_sets_the_duration` |
| P-MF-04 | tempo 为 0 的事件忽略；SMPTE division 按帧率和每帧 tick 数换算 | GM | `midi/tests.rs`: `tempo_of_zero_is_ignored`、`smpte_division_counts_frames_whatever_the_tempo`、`tempo_changes_set_the_time` |
| P-MF-05 | 0xF1–0xFE 的状态字节（system common、realtime）报错 | GM L6 | `midi/tests.rs`: `system_common_and_realtime_statuses_fail` |
| P-MF-06 | 曲长有上界（例如 24 小时），超出报错；tick 用 u64 累加 | GM M3 | `midi/tests.rs`: `files_over_a_day_fail`、`ticks_add_up_without_overflowing` |
| P-MF-07 | 轨道合并是 O(n log n)；65535 条空轨道的文件在 100 ms 内加载 | GM M2 | `midi/tests.rs`: `many_empty_tracks_load_fast`、`many_tracks_of_notes_merge_in_order` |
| P-MF-08 | 没有循环起点标记时，整曲循环回到时间 0 | GM H2 | `midi/tests.rs`: `whole_file_loop_goes_back_to_time_zero`、`loop_without_start_marker_starts_at_zero` |
| P-MF-09 | 回卷时保留越过终点的时间，循环不漂移；每次回卷先释放文件用到的通道上的音，再复位这些通道、恢复循环起点的状态，循环里改过的音量、声像、program 等第二圈起复原；循环起点 tick 上的 GM/GS 复位第二圈起由内部复位代替，不切断声部、不清延迟线 | GM | `midi/tests.rs`: `loops_keep_time_exactly_over_many_times_round`、`going_round_releases_every_note_first`、`going_round_sends_the_state_at_the_loop_start`、`going_round_undoes_what_the_loop_set`、`going_round_reaches_the_state_of_the_first_time_round`、`loops_shorter_than_a_millisecond_are_no_loops`、`a_reset_at_the_jump_target_takes_the_place_of_the_reset_of_the_channels`、`a_reset_at_the_loop_start_cuts_no_notes_and_clears_no_effects` |
| P-MF-10 | `seek` 和回卷之后，synth 上文件用到的通道的状态（CC、bank、program、preset、drum、弯音和弯音范围、触后、RPN/NRPN 的寄存器和生成器偏移）和 bank select 方式与从头播到目标处相同，目标 tick 上有复位消息时也一样；文件没用到的通道（宿主的）不受跳转影响；CC121 和 sysex 的判定同 synth，SoundFont NRPN 先 LSB 后 MSB，rhythm part 与通道事件的先后不变 | 设计 | `midi/tests.rs`: `seeking_reaches_the_state_played_from_the_start`、`going_round_reaches_the_state_of_the_first_time_round`、`a_reset_where_a_jump_lands_gives_the_state_it_gives_from_the_start`、`jumps_leave_the_channels_the_file_does_not_use_alone`、`the_channels_a_file_uses`、`seeking_back_undoes_what_was_set_after`、`seeking_keeps_the_low_bits_of_soundfont_nrpns`、`the_chase_resets_all_controllers_as_the_synthesizer_does`、`the_chase_judges_system_exclusive_messages_as_the_synthesizer_does`、`the_chase_keeps_rhythm_parts_in_their_place`、`seek_sends_the_state_and_no_notes`、`seek_forgets_what_a_reset_undid`；`synth::queue::tests::frames_then_order_of_queueing` |
| P-MF-11 | `Sequencer` 的采样率取 synth 的，范围同 `Config`（8000–192000）；`render` 遇到不同采样率的 synth 时按 synth 的采样率接着播 | 设计 | `midi/tests.rs`: `sample_rates_are_a_synthesizers`、`render_plays_at_the_synthesizers_rate` |

## 引擎（`synth`）

| 编号 | 行为 | 出处 | 测试 |
| --- | --- | --- | --- |
| E-01 | 事件在 `schedule` 指定的帧生效，误差 0 帧（工作预算用完时除外，见 E-18） | 设计 | `synth::tests::e01_events_take_effect_at_their_frame`、`e01_the_output_does_not_depend_on_the_render_lengths`、`e01_sent_and_late_events`、`controllers_change_playing_voices`；`midi::tests::render_plays_bursts_on_time_through_a_small_queue` |
| E-02 | scaleTuning 只缩放 (key - rootKey)；弯音、LFO、包络对音高的作用和调音不缩放 | SF §8.1.3, GM H4 | `synth::tests::e02_scale_tuning_scales_the_key_only` |
| E-03 | 生成器叠加和调制后按规范范围钳制（例如衰减不小于 0） | SF §8.1.3, GM L1 | `synth::tests::e03_modulated_generators_are_clamped` |
| E-04 | 默认调制器和区域调制器按规范合并 | SF §8.4, §9.5, GM H3 | `synth::tests::e04_modulators_merge` |
| E-05 | exclusive class：同一通道、同一 preset 内，新音停掉同类的旧音（含已在释音、被踏板保持的音），用 -2000 tc（约 315 ms）的快速释音，同 FluidSynth；同一个音的多个区域同类时一起发声 | SF §8.1.3, GM, FS | `synth::tests::e05_exclusive_classes`、`e05_exclusive_classes_stop_held_and_released_notes`、`e05_exclusive_classes_stay_in_their_channel_and_preset` |
| E-06 | 抢占和 All Sound Off 不硬切，约 2 ms 淡出 | GM M7 | `synth::tests::e06_stolen_voices_fade_out`、`stealing_prefers_released_voices` |
| E-07 | NoteOff 和 NoteOn 对超出范围的 key 做同样的处理 | GM M5 | `synth::tests::e07_note_off_finds_the_note_on_out_of_range` |
| E-08 | 一个音持续几十小时，计数不溢出，输出没有非有限值 | GM M4 | `synth::tests::e08_fifty_hours` |
| E-09 | 安静的音也按 send 送入混响和合唱 | GM | `synth::tests::e09_quiet_notes_reach_the_effects` |
| E-10 | 循环比一次读取步长还短时，在循环内取模，不读循环外的数据 | GM | `synth::tests::e10_short_loops_stay_in_the_loop`；`synth::dsp::oscillator::tests` |
| E-11 | modLfoToVolume 为负时，颤音同样有效 | GM | `synth::tests::e11_negative_mod_lfo_to_volume_works` |
| E-12 | 鼓通道找不到 bank 的 preset 时，先退到 128:program，再退到 128:0 | GM | `synth::tests::e12_drum_kits_fall_back` |
| E-13 | Bank select 在下一次 program change 时才生效 | GS, GM L3 | `synth::tests::e13_bank_select_waits_for_the_program_change`、`gm2_bank_select_switches_drums` |
| E-14 | NRPN 不改变之前选中的 RPN | GM | `synth::tests::e14_nrpns_leave_the_rpn_alone` |
| E-15 | GS reset、GM reset 复位所有通道；GS 的 use-for-rhythm-part 切换鼓通道 | GS, GM L2 | `synth::tests::e15_resets_and_rhythm_parts` |
| E-16 | 渲染路径不分配内存 | 设计 | `tests/memory.rs`：`playing_allocates_nothing`、`sequencing_allocates_nothing` |
| E-17 | 声部开始后至少过 `min_note_length`（默认 10 ms，含 delayVolEnv）才释音：更早的 NoteOff 或踏板抬起推迟到那一帧释音；更晚的不受影响；为 0 时同一帧的 NoteOn+NoteOff 无声 | FS | `synth::tests::short_notes_sound_the_minimum_length` |
| E-18 | 事件的工作受按帧补充的预算约束：代价超出时事件按顺序顺延（在 64 帧的网格上放行）、全部生效，吞吐不随采样率和复音数塌缩；队列满时 `send` 只多做一个事件；Sequencer 给宿主留八分之一的队列；`send` 排在等着的事件之后；正常的和弦（256 个音同一帧）准时；何时生效不随 `render` 的调用长度变化；Sequencer 单次 render 不会处理完同一时刻的大量事件 | 设计 | `synth::tests::a_flood_of_costly_events_plays_late_in_order`、`chords_play_within_the_budget`、`e01_waiting_for_the_budget_does_not_depend_on_the_render_lengths`、`e01_dense_events_under_the_budget_do_not_depend_on_the_render_lengths`、`waiting_events_drain_at_the_budget_rate`、`sending_to_a_full_queue_does_one_more_event`、`waiting_events_are_not_silence`；`midi::tests::a_crowd_of_events_at_once_plays_late_in_order` |
| E-19 | 输出乘以 `Config::gain`；gain 须为有限的非负数，`min_note_length` 不超过 1 s；交错输出的奇数尾样本清零 | 设计 | `synth::tests::the_gain_scales_the_output`、`config_ranges_are_checked`、`interleaved_odd_lengths` |
