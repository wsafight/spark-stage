# 质量与验证

## 1. 质量模型

SparkStage 把“代码路径存在”“后端可调用”“样本通过质量门”和“可作为默认能力”分开：

| 层级 | 含义 |
| --- | --- |
| 实现 | Rust 模块、命令和离线测试覆盖了路径 |
| 可用未验证 | adapter binding 与 ComfyUI schema 存在，但没有该操作的真实硬件证据 |
| 已验证 | 当前机器、workflow、模型和 profile 完成独立 smoke，媒体硬检查和血缘证据齐全 |
| 默认准入 | 已验证样本之外，还完成足够 seed、人工审片和性能/质量复核 |

任何一层都不能代替下一层。尤其是 `/object_info` 返回节点，不等于该节点在当前模型和工作流中质量可用。

## 2. 媒体硬检查

每个生成输出在从 staging 晋级为 raw take 前执行：

- 视频流可解码；
- 时长在期望值容差内；
- 需要音频时存在有效音轨；
- 黑帧低于门限；
- `freezedetect` 检出的静帧占比低于 profile 门限；
- 音轨没有异常长静音；
- 首帧、末帧和 handoff 候选帧能够抽取。

当前静帧策略：`freezedetect=n=-50dB:d=1.5`，闭合区间和延伸到 EOF 的开放区间都会汇总。adapter 配置的 `media_check_profiles` 设置 `max_freeze_ratio`：

```yaml
media_check_profiles:
  baseline: { max_freeze_ratio: 0.30 }
  audition: { max_freeze_ratio: 0.30 }
  final: { max_freeze_ratio: 0.20 }
```

profile 名称必须对应生成 profile，比例必须在 `0.0..=1.0`。没有显式策略时默认 30%。

## 3. 当前 DGX 证据

### T2V：已认证

见 [h3-t2v-2026-08-29.md](evidence/h3-t2v-2026-08-29.md)：

- MiniMax H3、ComfyUI 0.32.0、DGX Spark GB10；
- 960x544、24 fps、124 帧、20 steps；
- 5.167 秒 H.264 + 32 kHz AAC stereo；
- 端到端约 455.108 秒；
- decode、duration、audio、black、freeze、silence 和边界帧均通过；
- job、take、workflow、model、seed 和 backend lineage 完整。

### 12 步 audition：未准入

同 prompt、shape、模型和固定 seed 的三次实验约 267.5–267.9 秒，平均比 20 步快约 41%。但 seed=42 有 1.875 秒静帧，占 36.3%，超过当前 30% audition 门禁。

这组样本只证明“12 步值得继续实验”，不能证明“12 步可以替换正式 profile”。历史实验使用旧的 95% 门禁，不能被改写；新的回归测试验证未来会拒绝同类样本。

### R2V：conditioning 运行成功但质量失败

见 [h3-r2v-2026-08-29.md](evidence/h3-r2v-2026-08-29.md)：参考图经过项目导入、上传和动态 `LoadImage -> ref_images.ref_image_0` 接入，ComfyUI 成功生成了 5.167 秒输出，但末段 1.917 秒静帧，占 37.1%，被 audition 门禁拒绝。

因此 R2V 仍是 `available_unverified`，不能因为 conditioning 节点工作就写成 `verified`。

## 4. 质量门与人工责任

机器硬检查可以筛除明显技术坏片，但不能可靠回答：

- 人物是否保持身份和表演意图；
- 镜头是否真的符合故事和运镜；
- 中文对白是否说对、口型是否同步；
- 是否出现提示词没有描述的内容安全问题；
- 相邻镜头是否在服装、道具、视线和空间方向上连续。

所以所有候选都通过时仍需要用户选择；终片即使机器检查全部通过，也要经过 `final_visual_review`，之后才能进入 `playable` / `done`。

## 5. Benchmark 规则

Benchmark run 是不可变的实验记录，不是第二个投递客户端。每个 profile 必须记录：

- workflow / adapter / model fingerprint；
- 分辨率、fps、帧数、steps、sampler、scheduler、seed；
- 冷启动与稳态时间；
- 阶段耗时、显存和系统负载；
- 原始 job、take、媒体探针和证据路径；
- accepted、rejected 或 needs_more_data 结论。

一次样本只能证明一次样本。模型、节点、PyTorch、CUDA、驱动或 workflow 改变后，必须建立新的 baseline。

## 6. CI 与本地复现

本仓库的 GitHub Actions 和本地质量门保持相同原则：

```bash
cargo +1.98.0 fmt --all --check
sh scripts/check-rust-file-size.sh
cargo +1.98.0 test --all-targets --all-features
cargo +1.98.0 clippy --all-targets --all-features -- -D warnings
sh scripts/evaluate-script-bundles.sh
sh scripts/check-docs.sh
```

CI 还会执行 line coverage、cargo audit、cargo deny 和 Linux aarch64 check。硬件 smoke 证据不在普通 CI 中伪造；它必须在目标 DGX Spark 上单独记录。
