# 系统架构

## 1. 分层结构

```text
┌─────────────────────────────────────────────────────────────┐
│ 客户端层：CLI / Ratatui TUI / 外部 Agent / 只读审片客户端     │
└──────────────────────────────┬──────────────────────────────┘
                               │ Unix socket
┌──────────────────────────────▼──────────────────────────────┐
│ 控制层：worker、command journal、revision、审批、通知         │
└───────────────┬──────────────────────┬──────────────────────┘
                │                      │
┌───────────────▼──────────────┐ ┌─────▼───────────────────────┐
│ 持久层：JSON / JSONL / locks  │ │ 调度层：GPU queue / budget  │
└───────────────┬──────────────┘ └─────┬───────────────────────┘
                │                      │
┌───────────────▼────────────────────▼────────────────────────┐
│ 执行层：ComfyUI camera adapter、FFmpeg build executor         │
└───────────────┬────────────────────┬─────────────────────────┘
                │                    │
       ┌────────▼────────┐  ┌────────▼────────┐
       │ ComfyUI / H3    │  │ ffmpeg / ffprobe │
       └─────────────────┘  └─────────────────┘
```

这是一个 Cargo package，而不是分布式服务。Unix socket 只监听本机；远程 ComfyUI endpoint 需要配置显式允许。Rust 主程序保持薄入口，领域逻辑由 CLI、worker、TUI 和测试共同调用。

## 2. 核心模块

| 模块 | 职责 |
| --- | --- |
| `domain` | Project、Shot、Job、Take、Build、Reference 和审批模型 |
| `validation` | ScriptBundle schema、跨字段关系、内容边界和安全路径 |
| `store` | 原子 JSON、JSONL、SHA-256、文件锁、迁移、项目归档 |
| `worker` | 单写者命令处理、GPU 调度、预算、恢复和通知 |
| `adapter/comfy` | API workflow binding、上传、WebSocket、history 和输出收取 |
| `media` | ffprobe、黑帧/静帧/静音探针、边界帧抽取 |
| `build` | draft / trailer / final recipe、FFmpeg 拼片、字幕和联系表 |
| `benchmark` | 不可变 H3 run 元数据和已有 job 样本导入 |
| `tui` / `cli` | 共用 IPC 命令面的终端客户端 |

## 3. 一次生成任务的数据流

```text
ShotContract
    │ prompt / seed / profile / conditioning
    ▼
JobJournal（先落盘）
    ▼
AttemptJournal（request id）
    ▼
ComfyAdapter::prepare
    ├── 校验本地 workflow 和 object_info
    ├── 写入 declared bindings
    ├── 安全上传参考图或参考视频
    └── 记录 workflow hash
    ▼
ComfyUI /prompt
    ▼
WebSocket progress + /history reconcile
    ▼
下载到 raw/<shot>/.staging
    ▼
inspect_with_policy + extract_boundaries
    ├── 失败：保留 staging，Job = failed
    └── 通过：原子 rename，创建 TakeMetadata
```

WebSocket 只负责进度，不能单独决定成功。连接断开时 adapter 使用 `/history` 收口；提交阶段如果无法确认 backend id，则进入未知状态而不重复提交。

## 4. 项目目录与机器目录

项目目录默认位于应用数据根的 `projects/<project-id>/`，与源码仓库分离：

```text
project/
├── PROJECT.md
├── bible/                  # 人物、地点、风格来源
├── script/                 # brief、story、shots、authoring receipt
├── refs/                   # 不可变参考资产
├── state.json              # 当前状态快照 + 单调 revision
├── decisions.jsonl         # 人工 / 策略决策
├── events.jsonl            # 机器事件
├── jobs/                   # 逻辑 job journal 和 attempts
├── raw/<shot>/             # 已验证 take 与 staging 失败产物
├── review/<shot>/          # 首帧、末帧、handoff 候选
├── builds/<build-id>/      # recipe、字幕、联系表、review report
└── final/                  # 交付副本
```

机器级目录保存 `queue.json`、`worker.sock`、`worker.lock`、command journal 和 benchmark 原始元数据，不复制完整提示词、视频或参考素材。

## 5. 状态与恢复

状态写入采用“读取当前 revision → 校验命令 → 原子写入新快照”的模式。变更命令必须携带 `expected_revision`；冲突时返回当前 revision，不重放旧命令。

一个 camera job 的恢复状态可简化为：

```text
queued
  → active / prepared
  → submitting
  → submitted / running
  → succeeded → media checking → take created
                         └──────→ output_invalid / failed
```

worker 启动时重建机器队列，并重新检查项目合同 hash、adapter fingerprint、job 状态和输出文件。已提交但没有 backend id 的任务不会自动重试，避免同一镜头生成两次却无法对账。

## 6. 参考素材与动态 binding

参考素材先由项目 store 导入，生成 `REF-*` ID、相对路径、文件大小和 SHA-256。Comfy adapter 上传前拒绝绝对路径、路径穿越、symlink 和项目根之外的文件。

H3 当前参考图路径通过动态 autogrow 输入构成：

```text
项目参考图
   ↓ upload/image
ComfyUI LoadImage 节点
   ↓ [node_id, 0]
ref_images.ref_image_0
```

参考视频则先构造 `LoadVideo -> GetVideoComponents`，再把视频帧流接到 `ref_videos.ref_video_0`。这些辅助节点在 preflight 中按 `/object_info` schema 校验，不依赖“节点名字看起来存在”。

## 7. 构建与交付

Build 不改 raw。`BuildRecipe` 冻结镜头顺序、take id、trim、profile、输入 hash、参考 fingerprint、字幕 cue 和输出路径；worker 执行时重新核对这些身份。

FFmpeg 先写 staging，再执行媒体检查，最后原子发布输出和 delivery 副本。含对白的 build 同时生成 SRT 与 VTT；联系表和 `review-report.json` 与 recipe 一起保存，保证交付文件可以回溯到每个镜头。
