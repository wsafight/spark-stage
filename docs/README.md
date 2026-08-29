# SparkStage 文档

SparkStage 是运行在 NVIDIA DGX Spark 上的本机优先 AI 制片控制面：它把外部 Agent 生成的结构化剧本合同，交给本机 ComfyUI / MiniMax H3 逐镜生成，再通过质量检查、人工决策和确定性拼片交付视频。

## 从这里开始

| 文档 | 回答的问题 |
| --- | --- |
| [项目总览](overview.md) | SparkStage 是什么，服务谁，完整流程如何走 |
| [原理分析](principles.md) | 为什么采用合同、单镜、worker、不可变证据和人工闸门 |
| [系统架构](architecture.md) | 模块如何分层，数据如何流动，状态如何恢复 |
| [运行手册](operations.md) | 如何从 brief 开始，排队、审片、重试、拼片和归档 |
| [质量与验证](quality.md) | 哪些能力已经验证，质量门如何工作，哪些结论不能外推 |

## 设计依据

- [产品文档](product.md)：产品边界、用户流程和不做事项。
- [技术设计](technical.md)：Rust 模块、持久化、IPC、worker 和 adapter 约束。
- [P0 功能清单](p0-features.md)：控制面能力及验收结果。
- [H3 优化计划](optimization.md)：性能实验方法和准入标准。
- [DGX T2V 证据](evidence/h3-t2v-2026-08-29.md)：已认证的真实本机 T2V 样本。
- [DGX R2V 证据](evidence/h3-r2v-2026-08-29.md)：conditioning 已运行但因质量门失败的样本。

## 一句话理解

```text
外部 Agent
    │  ScriptBundle（结构化剧本合同）
    ▼
校验与审批 ──→ worker / project store ──→ GPU 队列
                                           │
                                           ▼
                              ComfyUI / MiniMax H3 adapter
                                           │
                                           ▼
                         视频、音频、探针、边界帧和血缘
                                           │
                                           ▼
                              候选决策 ──→ build / 交付
```

本文档集描述的是当前仓库中的真实实现。任何硬件能力、模型能力或性能结论，都以对应证据文档为准，不从节点存在或配置文件推断已验证。
