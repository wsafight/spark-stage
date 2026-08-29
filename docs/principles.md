# 原理分析

## 1. 合同优先，而不是 prompt 优先

自然语言 prompt 适合表达意图，不适合作为生产数据库。仅从 prompt 推断人物、地点、台词或连续性，会导致同一个角色在不同镜头中出现多个隐含版本。

SparkStage 把创意拆成稳定字段：

- `bible` 管人物、地点和风格来源。
- `shots` 管镜头规格、角色、地点、相机、操作类型和对白。
- `conditioning` 明确首帧、尾帧、参考图或参考视频。
- `generation_plan` 明确 audition / final profile 和候选数量。
- `continuity` 明确跨镜头状态，而不是让 worker 猜测。

因此 prompt 是合同中的一个输入字段，而不是系统唯一的事实来源。校验器可以在进入 GPU 前发现未知角色、错位地点、非法参考路径、I2V 缺首帧和 audition/final profile 冲突。

## 2. 单镜是原子，成片是投影

整部视频作为一次任务会放大失败成本：一个镜头脸漂了，用户只能重新生成整部片子；中途重启，也很难知道已经完成了什么。

SparkStage 把 shot 作为原子生产单位：

```text
ShotContract ──→ JobJournal ──→ AttemptJournal ──→ TakeMetadata
                                                        │
                                                        ▼
                                                BuildRecipe 的输入
```

`raw` take 一旦通过输出验证就冻结。draft、trailer 和 final 不是修改 raw，而是从已选 take 按 recipe 重新投影。这样可以只重拍 S06，也可以在不重新生成的情况下生成局部 draft。

## 3. 控制面与执行面分离

CLI 和 TUI 都可能崩溃、重复发送或同时打开。让它们直接改 `state.json` 会产生竞态，也无法集中处理 revision conflict。

系统采用单写者模型：

```text
CLI / TUI / 外部客户端
          │ Unix domain socket + command_id + expected_revision
          ▼
       worker
       ├── project store（状态、决策、job）
       ├── scheduler（GPU 独占、暂停、预算）
       ├── camera executor（ComfyUI）
       └── build executor（ffmpeg）
```

worker 负责检查当前 revision、写 prepared / committed 决策、更新队列和恢复状态。客户端只发命令和读取快照，因此 TUI 换掉不会产生第二套业务逻辑。

## 4. 不可变证据优于“当前看起来成功”

AI 生成结果会受模型、节点、驱动、seed 和参考素材影响。只保存一个 mp4 文件不能回答“它是怎么生成的”。

每个 take 至少关联：

- `job_id`、`request_id` 和 ComfyUI backend job id；
- 合同和输入 hash；
- adapter、workflow 和 model fingerprint；
- seed、profile、操作类型和父 take；
- ffprobe / ffmpeg 质量检查；
- 首帧、末帧、handoff 候选和参考素材 fingerprint。

文件本身也通过 SHA-256 和安全的项目相对路径关联。项目归档使用逐文件 hash manifest；导入先校验再发布，永不覆盖已有项目。

## 5. 质量门是晋级条件，不是装饰指标

生成成功只说明后端返回了文件，不说明文件能交付。SparkStage 将输出分为几个明确阶段：

1. **解码与规格**：容器、视频流、分辨率、fps、时长和音轨。
2. **明显缺陷**：黑帧、静帧和异常静音。
3. **候选决策**：机器检查通过后，用户选择或批准 take。
4. **成片检查**：build 再次探针，生成字幕、联系表和报告。
5. **人工终审**：机器不能凭 prompt 判断故事、人物身份和画面安全；终片在人工确认前只能是 `needs_review`。

静帧门禁按生成 profile 配置。当前 H3 的 `audition` 允许总时长的 30%，`final` 允许 20%。这个阈值直接拦截了 R2V 和 12 步实验中观察到的冻结样本，避免“速度更快”掩盖“画面不动”。

## 6. 能力声明必须由证据驱动

ComfyUI 的 `/object_info` 能证明节点安装和输入 schema 存在，但不能证明该操作在当前 workflow、模型和机器上质量可用。因此能力有四个状态：

```text
unavailable → unsupported → available_unverified → verified
```

- `unavailable`：后端不可达或节点校验失败。
- `unsupported`：合同需要的 binding 不存在。
- `available_unverified`：binding 存在，但没有该操作的真实 smoke 证据。
- `verified`：当前 workflow 和硬件上完成了独立 smoke，并通过质量准入。

T2V 的证据不能自动认证 R2V、I2V 或 FLF2V；12 步 audition 的速度结果也不能自动替换 20 步正式 profile。

## 7. 本机优先是数据边界，也是故障边界

视频、音频、参考图和失败产物默认只写项目目录。机器级目录只存队列、锁、诊断和 benchmark 元数据，不复制项目媒体。外部 Agent 只负责文本合同，不读取 raw 和参考图。

本机优先同时减少了云端上传风险和网络不确定性，但意味着 GPU、ComfyUI、FFmpeg 和磁盘都是本机责任。preflight、磁盘硬线、GPU 独占锁和可恢复清理正是为了把这些责任显式化，而不是假装它们不存在。

## 8. 为什么不先做“大而全”的自动审片

自动语义、人物相似度和运动轨迹检查很有价值，但它们必须有版本、阈值、证据帧和校准样本。在这些条件满足以前，SparkStage 只把可确定的媒体检查作为硬门，把创意判断留给人。

这是一种保守的晋级策略：宁可多一次人工确认，也不把单一 VQA 分数当作“镜头已通过”。
