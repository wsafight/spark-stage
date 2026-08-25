# SparkStage MiniMax H3 性能与质量优化验证计划

**版本**：0.2<br>
**日期**：2026-08-26<br>
**状态**：待基线实测<br>
**适用环境**：NVIDIA DGX Spark（GB10）+ 本机 MiniMax H3 + ComfyUI<br>
**对应产品文档**：[product.md](product.md)

---

## 1. 文档目标

这份文档只回答一件事：怎样在 DGX Spark 上让 MiniMax H3 的出镜链路更快、更稳，同时能证明画质、人物一致性和音画同步没有被不可接受地破坏。

当前已知对照组是 **5 秒 / 960x544 / 12 步约 4 分钟**。这个数字只作为待复测起点，不直接当成优化结论。所有候选方案都必须在同一套 workflow、分镜、输入、seed 组和输出规格下与基线对照。

这不是安装清单。一个包能安装、一个 ComfyUI 节点能加载，都不等于 MiniMax H3 的实际 workflow 已经使用它。只有完成调用链确认、性能测量和质量复核，能力才能标为 `verified`。

## 2. 优化原则

1. **先测量，再改动**：先定位耗时在文本编码、DiT、VAE、音频还是数据搬运。
2. **一次只改一个主变量**：attention、compile、cache、量化分别建 profile，不在第一次实验中叠加。
3. **速度不能覆盖硬质量失败**：黑帧、静帧、换脸、意外跳切、对白缺失或明显不同步时，提速结果直接淘汰。
4. **冷启动和稳态分开记录**：`torch.compile` 等方案必须同时报告首次运行和重复运行。
5. **自动分只做筛查**：最终晋级由盲评或人工审片决定，不能只看单一 VQA 分数。
6. **显存方案不冒充加速方案**：tiling 和 offload 首先解决可运行性，若墙钟变慢要如实记录。
7. **优化绑定具体环境**：任何结论必须带 workflow hash、模型栈、节点版本、PyTorch、CUDA 和驱动信息。
8. **benchmark 不绕过生产控制面**：所有实验通过 `sparkstage benchmark h3` 进入同一 worker、GPU 资源锁和 camera adapter；profiling 只增加观测，不复制投递与恢复实现。

当前 P0 的 `init/record/show` 只管理不可变 run 元数据和已有生产 job 的样本导入，不会直接投递 ComfyUI。真实实验执行仍必须由 worker 取得 `gpu_benchmark` reservation 后调用生产 adapter；不能把离线 `record` 当成运行或验证证据。

## 3. 优先级

### P0：先把测量和控制面做对

| 目标 | 工具 / 库 | SparkStage 中的作用 | 验收结果 |
| --- | --- | --- | --- |
| 工作流控制 | SparkStage worker + ComfyUI API、WebSocket、`/history` | 复用生产 adapter 自动投递、监听进度、按原 job id 恢复、定位和下载结果 | benchmark 与生产任务共享队列和恢复语义 |
| 媒体检查 | ffmpeg、ffprobe | 检查时长、帧率、音轨、黑帧、静帧、静音和响度 | 每个 take 产出结构化探针结果 |
| 性能测量 | `torch.profiler`、NVIDIA Nsight Systems、`nvidia-smi` | 判断时间消耗和 CPU / GPU / 内存瓶颈 | 能给出阶段耗时、峰值内存和利用率时间线 |
| 视频封装 | ComfyUI-VideoHelperSuite | 输入视频、帧序列、音频和输出封装 | 输入输出节点通过当前 workflow 烟测 |
| 通用节点 | ComfyUI-KJNodes | 图像缩放、帧处理、批处理和工作流辅助 | 只启用 workflow 实际依赖的节点 |

### P1：再做推理提速对照

- PyTorch SDPA 稳定基线
- SageAttention 与 SDPA 对照
- `torch.compile` 冷启动 / 热启动对照
- 一次只测一种 cache
- 逐模块 FP8 / INT8
- 为可运行性准备 VAE tiling、CPU offload、sequential offload

### P2：建立自动审片和连续性量化

- Qwen3-VL + OpenCV 作为第一批通用检查
- 人脸 embedding、镜内跳切、感知偏差和运动轨迹作为专项检查
- FunASR + VAD + ffmpeg 作为中文对白第一批检查

### P3：最后考虑后期增强

- RIFE / FILM 补帧
- 时序视频超分
- 显式可选的人脸修复
- 统一色彩空间和交付编码

## 4. 环境指纹

每次 benchmark 在启动前冻结以下信息；任一关键项变化都建立新基线，不能和旧结果直接混算。

| 类别 | 必须记录 |
| --- | --- |
| 硬件 | DGX Spark 型号、GPU / 统一内存可用量、功耗模式、温度区间 |
| 系统 | OS、内核、NVIDIA 驱动、CUDA、cuDNN |
| Python 栈 | Python、PyTorch、xformers / attention 后端及实际加载状态 |
| ComfyUI | commit、启动参数、custom node 名称与 commit、缺失节点 |
| H3 | 模型文件清单、文件 hash、精度、文本编码器、VAE、音频模块 |
| workflow | API workflow JSON、workflow hash、节点与输入绑定表 |
| 运行条件 | profile、分辨率、帧数、fps、步数、sampler、scheduler、seed、参考输入 |
| 系统负载 | 测试前可用内存、后台 GPU 进程、磁盘剩余空间和输出盘 |

禁止只记录一个笼统的“MiniMax H3 版本”。同一模型权重在节点、CUDA 或精度变化后可能产生不同速度和结果。

## 5. 基准样本

固定一组不随优化改写的镜头合同，至少覆盖：

| 编号 | 场景 | 主要压力 |
| --- | --- | --- |
| B01 | 单人近景、中文对白、轻微表情 | 脸、口型、原生音频 |
| B02 | 双人正反打中的单镜 | 人数、身份和视线 |
| B03 | 人物走位与手部动作 | 运动细节和肢体稳定 |
| B04 | 明显推拉或横移运镜 | 时间一致性和背景结构 |
| B05 | 低照度雨夜室内 | 曝光、噪声和暗部闪烁 |
| B06 | 首帧 / 尾帧或参考视频约束 | conditioning 是否真实生效 |

每个 profile 使用相同输入和至少 3 个固定 seed。涉及 compile 时，另做同一形状连续运行组。正式比较前先用短任务预热机器，避免把首次模型加载误当成稳定推理时间。

## 6. 实验方法

### 6.1 单变量矩阵

第一轮 profile 必须互相隔离：

| Profile | 相对基线唯一主变化 | 要回答的问题 |
| --- | --- | --- |
| `baseline` | PyTorch SDPA，当前可稳定运行的精度 | 当前真实对照是多少 |
| `sage` | attention 换为 SageAttention | H3 DiT 是否真正接入、是否提速、是否改画质 |
| `compile` | 在 baseline 上只启用 `torch.compile` | 编译成本跑多少次后回本 |
| `cache-<name>` | 一次只启用 TeaCache、First Block Cache、DeepCache 或 PAB 中的一种 | 节省多少 DiT 计算，动作和口型损失多少 |
| `fp8-<module>` | 一次只量化一种模块 | 哪个模块降精度可接受，瓶颈是否缓解 |
| `offload-<name>` | 一次只启用一种 offload / tiling | 是否从不可运行变为可运行，代价多少 |
| `audition` | 降低经实测有效的采样成本 | 2–3 个候选是否比一个 final 更便宜 |
| `final` | 当前批准的交付规格 | 作为小样晋级和最终交付基准 |

FlashAttention 只有在 H3 节点确认接入对应 kernel 时才单列 profile。不能因为环境里安装了包，就把它记为已启用。

第二轮才允许组合已经单独通过的选项。组合 profile 仍要有明确父基线，例如 `sage+fp8-dit`，并重新跑完整质量闸门，不能假定收益相加、损伤不相加。

### 6.2 每轮运行顺序

1. 做 preflight，写入环境指纹和可用能力。
2. 向 worker 申请 benchmark reservation，等待当前 GPU job 到达安全边界，并阻止新的独占任务进入；不得清理、终止或绕过队列中的生产任务。
3. 保存 ComfyUI prompt / job id，通过 WebSocket 监听状态，并用 `/history` 收口最终状态。
4. 记录端到端墙钟、节点或阶段耗时、峰值内存、GPU 利用率、失败与重试。
5. 对输出执行媒体硬检查，再执行视觉、人物、运动和音频检查。
6. 生成匿名联系表，由人做 A/B 盲评。
7. 写结论为 `accepted`、`rejected` 或 `needs_more_data`，不覆盖原始结果。

### 6.3 必须分开的时间

- 模型首次加载
- graph / kernel 首次编译
- 文本编码
- DiT 去噪
- VAE decode
- 音频生成或处理
- 帧序列与音频封装
- 文件写入与下载
- SparkStage 排队、上传和控制开销

若节点无法直接暴露阶段时间，先用 `torch.profiler` 和 Nsight 找瓶颈，再决定是否值得给 adapter 增加细粒度埋点。

## 7. 性能指标

每个 profile 至少报告：

- 冷启动端到端墙钟和稳态墙钟
- 每个阶段的绝对耗时与占比
- 峰值 GPU / 统一内存、平均与峰值 GPU 利用率
- 生成帧数、每帧耗时或等价吞吐
- 首次编译时间、重复运行节省和预计回本次数
- 成功率、超时率和重试次数
- 输出文件写入与封装耗时
- 相对 `baseline` 的速度比和内存变化

平均值不能掩盖长尾。至少同时保留中位数、最慢一次和每次原始记录。样本量不足时标注为探索性结论。

## 8. 推理优化验证

### 8.1 Attention

**SDPA** 是稳定对照组。SageAttention 和 FlashAttention 分别回答三件事：H3 的目标 attention 模块是否实际调用了对应实现、Blackwell / 当前 CUDA 构建是否支持、输出质量是否通过闸门。

验收时必须保留调用证据，例如 profiler kernel、节点日志或模块级配置。只看到启动日志里出现包名不算通过。

### 8.2 `torch.compile`

只在固定五秒镜头、相同帧数和相同 shape 重复运行时优先测试。必须记录：

- 首次编译额外耗时
- 第二次及之后的稳态耗时
- shape 或 workflow 变化是否触发重新编译
- graph break、fallback 和缓存失效
- 连续生成多少镜后累计时间优于 baseline

若日常分镜频繁改变 shape，或回本次数高于一次典型项目的同形状镜头数，则不进入默认 profile。

### 8.3 Cache

TeaCache、First Block Cache、DeepCache、PAB 一次只测一种，并对高运动、人物近景和口型镜头分别评分。重点不是静态画面看起来是否相似，而是动作细节、时间一致性和发音期间的面部变化有没有损坏。

cache 阈值或跳步强度也是实验变量。同一轮不能同时改 cache 类型和强度。

### 8.4 FP8 / INT8 与 `torchao`

文本编码器、DiT、VAE、音频模块分别测量，不默认使用同一种精度。每次记录被量化的具体模块、权重 / 激活精度、fallback 算子、显存变化和耗时变化。

量化后若身份、肤色、暗部层次、口型或音频明显变差，即使没有数值错误也不通过。

### 8.5 Tiling 与 Offload

VAE tiling、CPU offload、sequential offload 的首要指标是能否在目标规格稳定运行，其次才是速度。DGX Spark 使用统一内存，传统独显上的经验不能直接套用；需要特别观察页迁移、数据搬运、峰值内存和长尾延迟。

## 9. 人物与镜头一致性

处理顺序固定为：

1. 优先验证 H3 原生 T2V / I2V / FLF2V / R2V、首帧、尾帧、参考图或参考视频能力。
2. 固定过审定妆图、服装描述、场景状态、镜头状态和 seed。
3. 用已选视频、稳定首帧、稳定尾帧或接力帧重新生成。
4. 最后才评估通用 IP-Adapter、ControlNet 或 ReferenceNet。

IP-Adapter、ControlNet 和 ReferenceNet 只有在具体 H3 ComfyUI 节点实现了相应 conditioning，并通过干预对照证明输入确实影响结果后才能启用。“节点可加载”不能写成“模型支持”。

一致性检查至少包括：

- 定妆图与抽帧的人脸相似度
- 同一 take 内的人脸漂移
- 相邻镜头服装、道具、人物位置和视线
- 首帧 / 尾帧与输入约束的视觉偏差
- 接力帧附近是否存在运动模糊、眨眼、口型中间态或构图突变

首尾帧只在叙事需要时使用。连续动作从上一镜最后约 0.3 秒中选择稳定清晰帧作为候选接力帧，不机械使用文件最后一帧。

## 10. 自动审片

| 维度 | 工具 | 用法与边界 |
| --- | --- | --- |
| 语义符合度 | Qwen3-VL | 抽帧检查人数、服装、场景、画面文字和动作；输出证据帧，不直接批准成片 |
| 身份 | InsightFace / ArcFace | 比较定妆图和视频抽帧 embedding；代码与权重分别进入统一许可闸门 |
| 基础画质 | OpenCV | 清晰度、曝光、重复帧、运动量、边界帧和异常色块 |
| 意外跳切 | PySceneDetect | 检测单镜内部不应存在的场景切换 |
| 参考偏差 | LPIPS / DISTS | 比较首帧、尾帧和参考图；只用于相对变化 |
| 候选排序 | FAST-VQA / DOVER | 辅助排序多个 take，不作为唯一审批标准 |
| 运动与轨迹 | RAFT / CoTracker | 检查运动方向、人物轨迹、局部抖动和跨帧稳定性 |

自动检查结果必须带时间戳或帧号。只有总分、没有失败位置的报告不能指导重拍。

## 11. 中文对白与原生音频

第一套核对链路：

1. 用 ffprobe 确认音轨、采样率、声道、时长和视频时长关系。
2. 用 Silero VAD 检查对白是否缺失，以及开头结尾是否被截断。
3. 用 FunASR 转写中文对白，与分镜台词做字词和语义核对。
4. 用 faster-whisper / Whisper large-v3 做第二套转写，处理两套结果冲突。
5. 用 SyncNet 检查明显音画漂移；阈值先通过人工标注样本校准。
6. 用 ffmpeg 的 `loudnorm`、`silencedetect`、`atrim`、`apad`、`acrossfade` 完成响度、静音、长度和镜头衔接。

只有在需要分离原生对白、音乐或环境声时使用 Demucs。分离会产生伪影，分离后的音轨必须重新通过响度、静音和对白可懂度检查。

RIFE / FILM 改变视频帧数后，必须按原视频时长重新对齐音频并再次执行 VAD、时长和 SyncNet 检查，不能只补视频帧。

## 12. 后期增强

| 方案 | 目标 | 默认策略 | 许可准入 |
| --- | --- | --- | --- |
| RIFE / FILM | 补帧、改善低帧率运动 | 可选；不能用于掩盖错误动作 | 代码与具体权重分别核验，未知时只允许内部实验 |
| Real-ESRGAN | 低接入成本逐帧超分 | 实验；重点检查闪烁和纹理漂移 | 代码、模型来源和权重许可分别记录 |
| BasicVSR++ / RealBasicVSR / RVRT | 时序视频超分 | 后续评估；接入成本高但更符合视频一致性 | 每个实现及权重单独核验，不按模型家族推断 |
| CodeFormer | 脸部修复 | 默认关闭；容易改变身份 | 当前公开项目许可含非商用限制，商业 profile 禁止，除非另获授权 |
| GFPGAN | 脸部修复 | 默认关闭；显式启用且必须复审身份 | 代码和权重分别核验，不能沿用 CodeFormer 的许可结论 |
| libplacebo / FFmpeg 色彩滤镜 | 缩放、色彩空间和交付编码统一 | 在 final build 中固定并记录参数 | 记录发行构建、链接方式和启用组件对应许可 |

后期增强不能把原 take 的血缘覆盖掉。增强结果是新派生产物，要记录源 take、工具、模型、参数和审片结果。

所有第三方代码、模型和权重使用统一字段：`source`、`code_license`、`weights_license`、`allowed_use`、`verified_at`、`evidence`。状态分 `verified_internal`、`verified_commercial`、`restricted`、`unknown`；`restricted` 和 `unknown` 不能进入商业 profile，也不能因为能下载或能加载就视为已授权。

## 13. Audition 与 Final 优化

小视频抽卡成立的前提不是“小”，而是 **候选组总成本低于直接生成 final，且仍能可靠判断表演方向**。

### 13.1 成本闸门

默认检查：

```text
audition_ratio = N 个 audition take 的总 GPU 墙钟 / 1 个 final take 的 GPU 墙钟
```

- 3 个 audition 的 `audition_ratio <= 1.0` 才允许默认三抽。
- 若超过 1.0，先降低候选数，再评估更低规格 profile。
- v1 目标是 3 个 audition 的比值不高于 0.7。
- 即使满足成本闸门，候选高度同质也算失败，不能用无差异结果制造选择感。

同时记录人工等待时间、失败重试、存储占用和晋级重拍成本，避免只优化单次 GPU 时间却拉长整个制片流程。

### 13.2 小样是否有判断价值

低规格 profile 必须保留足够信息，让人判断：

- 人数与人物大体身份
- 构图、运镜和动作方向
- 表演节奏和对白时长
- 关键道具与场景关系
- 是否值得晋级

如果低分辨率或低步数让脸、手、口型和运动完全无法判断，这个 profile 即使很快也不能成为 audition 默认值。

### 13.3 晋级策略

按对小样表演的保留程度排序：

1. `enhance`：直接处理选中视频，最能保留原表演。
2. `video_reference`：以选中视频作为参考重新生成。
3. `frame_reference`：抽取稳定首帧 / 尾帧，以 I2V 或 FLF2V 重拍。
4. `seed_replay`：同 seed 用 final profile 重拍，只能视为方向参考。

除 `enhance` 外，晋级都生成新的 take 并重新审片。同 seed 提高分辨率或步数不保证保留构图、动作或表演，界面和报告不能把它描述成原小样的高清版本。

## 14. 质量与准入闸门

### 14.1 硬失败

出现任一情况，profile 不能因提速而进入默认配置：

- 文件不可解码、时长或 fps 不符合合同
- 黑帧、长静帧、严重重复帧或异常闪烁
- 应有音轨却缺失、长静音、对白截断或明显音画漂移
- 人数错误、主要人物身份明显改变、服装或关键道具违背分镜
- 单镜内出现意外跳切
- 动作方向错误、肢体严重损坏或口型不可接受
- conditioning 声称生效但干预对照无法证明

### 14.2 软评分

在没有硬失败后，再比较：

- 人工盲评偏好与通过率
- 身份相似度和帧间漂移
- 清晰度、曝光、运动稳定性和感知质量
- 台词核对、响度和同步分数
- 稳态速度、内存、成功率和成本

自动指标用于筛选和定位，不替代人工批准。任何拟进入默认配置的优化至少要通过固定基准集；影响人物、动作或音频的方案还要进入《雨夜公寓》三镜试拍做实际叙事复核。

## 15. 实验产物

每次运行默认写入机器级应用数据目录，而不是源码仓库或某个电影项目：

```text
$XDG_DATA_HOME/sparkstage/benchmarks/h3/<run-id>/
├── environment.json          # 环境指纹
├── workflow-api.json         # 实际投递的 workflow
├── run.json                  # profile、输入、seed、job id 和状态
├── timing.json               # 墙钟与阶段耗时
├── telemetry.csv             # GPU / 内存 / CPU 时间序列
├── media-probe.json          # ffprobe 与媒体硬检查
├── review.json               # 自动指标和失败帧
├── review.md                 # 人可读报告与人工结论
├── contact-sheet.jpg         # 匿名盲评联系表
├── profiler/                 # profiler / Nsight 产物
└── output/                   # 原始生成结果
```

原始记录追加保存。汇总表可以重新生成，但不能只保留平均值。进入默认配置时，在 adapter profile 中记录对应 benchmark run id，保证以后能追到批准依据。

## 16. 执行顺序

### 第一轮：建立可信基线

`SparkStage worker / adapter + ComfyUI API + WebSocket + /history + VideoHelperSuite + ffprobe/ffmpeg + torch.profiler/Nsight + nvidia-smi`

目标：先让生产 worker / adapter 跑通控制、恢复、媒体检查和分阶段性能记录；再确认当前 SDPA baseline，并测出 audition / final 的真实成本比。

### 第二轮：提速与自动审片

`SageAttention / SDPA 对照 + torch.compile + 一种 cache + 逐模块 FP8 + Qwen3-VL + OpenCV + FunASR`

目标：每项独立验证，只有单项通过后才做有限组合；建立人物、动作和中文对白的自动筛查。

### 第三轮：专项质量与后期

`InsightFace / ArcFace + PySceneDetect + LPIPS / DISTS + RAFT / CoTracker + RIFE + 时序超分`

目标：提高候选排序和连续性诊断能力；只有原始 take 已通过内容审片后才做后期增强。

## 17. 第一批待执行实验

1. 导出当前 H3 API workflow，建立节点绑定表和 hash。
2. 完成最小 worker、GPU 锁与 production camera adapter，交付 `sparkstage benchmark h3`。
3. 用 B01、B03、B05 各跑 3 个固定 seed，复测 SDPA baseline。
4. 分别记录模型加载、文本编码、DiT、VAE、音频、封装和文件写入耗时。
5. 建两个 audition 候选 profile，与 final 比较三抽成本和方向可判断性。
6. 验证 SageAttention 是否实际进入 H3 attention 调用链。
7. 用固定 shape 连跑 `torch.compile`，计算真实回本镜头数。
8. 选择一种 cache 做高运动、近景口型对照，不同时测试其它 cache。
9. 从 DiT 开始做一个逐模块 FP8 对照，不全栈一次量化。
10. 接入 ffprobe / ffmpeg、OpenCV、FunASR 的硬检查和证据帧报告。
11. 完成盲评后，只把有调用证据、速度收益、许可记录和完整质量数据的 profile 标记为 `accepted`。

在这十一项完成前，不默认启用多种加速叠加，不用人脸修复遮盖生成问题，也不把后期超分当作 final 晋级的唯一方案。
