# SparkStage 技术设计

**版本**：0.4<br>
**日期**：2026-08-29<br>
**状态**：MVP 控制面与 DGX/H3 T2V 闭环已验证；其余生成能力与性能基线待验证<br>
**目标平台**：NVIDIA DGX Spark（Linux aarch64）<br>
**Rust 基线**：rustc 1.98.0，edition 2024<br>
**产品合同**：[product.md](product.md)<br>
**优化验证**：[optimization.md](optimization.md)

---

## 1. 技术结论

SparkStage MVP 采用以下结构：

1. 一个 Cargo package，源码按模块拆分，产出一个 `sparkstage` 可执行文件。
2. 同一可执行文件提供 CLI、常驻 worker 和 Ratatui TUI，不写 Rust 单文件，不做桌面壳。
3. worker 是状态与队列的唯一写入者；CLI、TUI、Agent 和以后出现的审片页都是客户端。
4. 外部 Agent 按仓库中的编剧 skill 生成 `ScriptBundle`；SparkStage 只提供 schema、校验、原子导入和审批，不运行语言模型。
5. ComfyUI / MiniMax H3 是第一个 camera adapter，ffmpeg / ffprobe 是媒体运行时，不重写已经可用的 Python 推理链。
6. 项目状态使用带 schema 版本的 JSON、JSONL 和 job journal；MVP 不引入数据库。
7. WebSocket 只提供实时进度，ComfyUI `/history` 和媒体探针共同决定 job 是否真正完成。
8. 投递前先落盘 request，无法确认是否已提交时进入 `SUBMISSION_UNKNOWN`，不自动重复投递。
9. TUI 只做队列、状态和审批；视频预览调用外部播放器，完整视觉审片留给 v1 本机页面。

优先级是：**数据合同与恢复 > Agent 文案包导入 > ComfyUI 闭环 > 媒体检查 > audition/final > TUI > 自动审片增强**。

## 2. 范围与约束

### 2.1 MVP 必须完成

- 一条 H3 T2V workflow 的真实节点绑定和能力烟测
- 一条外部 Agent `ScriptBundle` 的结构化校验、原子导入和审批闭环
- 单机单 worker、单 GPU 视频任务
- 项目、shot、job、take、review 和 build 的持久状态
- CLI 和 Ratatui TUI 共用命令面
- ComfyUI 投递、监听、恢复、历史查询和输出收取
- ffprobe / ffmpeg 硬检查、抽帧、拼片和响度处理
- audition、候选选择、final 晋级和 direct render
- 原子写入、项目锁、机器级队列和崩溃恢复
- 《雨夜公寓》三镜试拍和十镜闭环

### 2.2 MVP 不做

- Tauri、云控制台、多人协作或远程裸 TCP API
- 终端内视频解码、时间线编辑器或逐帧调色
- 未经 workflow 烟测的 I2V、FLF2V、R2V 能力
- 用自动语义分数替代人工审片
- 为了“纯 Rust”重写 ComfyUI 节点或现有推理脚本
- SQLite、消息队列服务或分布式调度

## 3. 总体架构

```text
      用户 ──→ 外部 Agent + screenwriter skill
                         │ ScriptBundle / CLI commands
               ┌─────────┴─────────┐
               │                   │
          CLI commands         Ratatui TUI
               │                   │
               └─────────┬─────────┘
                         │ Unix domain socket
                         ▼
                 sparkstage worker
              唯一命令处理与状态写入者
       ┌─────────────────┼──────────────────┐
       ▼                 ▼                  ▼
 project store      global scheduler    pipeline engine
 JSON / JSONL       one GPU owner       script / review / build
       │                 │                  │
       └────────────┬────┴──────────┬───────┘
                    ▼               ▼
            ComfyUI adapter    ffmpeg / ffprobe
                    │
             MiniMax H3 workflow
```

worker 可以由 `sparkstage worker run` 前台运行，部署稳定后再用 systemd user service 托管。CLI 不在 worker 缺席时偷偷改状态；变更命令返回 `WORKER_UNAVAILABLE` 和明确启动动作。只读命令可以读取最后一次原子快照，并标记快照时间。

## 4. 源码结构

MVP 先保持一个 package，避免过早拆 workspace：

```text
spark-montage/
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── main.rs                 # 参数解析和进程入口
│   ├── lib.rs                  # 模块导出
│   ├── cli/                    # 人类输出与 --json 输出
│   ├── ipc/                    # 本机命令协议与 Unix socket
│   ├── worker/                 # 唯一写入者、恢复和生命周期
│   ├── domain/                 # project / shot / job / take / build
│   ├── store/                  # 原子 JSON、JSONL、锁和迁移
│   ├── queue/                  # 全局调度与资源锁
│   ├── adapters/
│   │   └── comfy/              # API、WebSocket、history 和绑定
│   ├── media/                  # ffprobe、ffmpeg、抽帧和封装
│   ├── review/                 # 硬检查与 review run
│   ├── build/                  # draft / trailer / final 配方
│   ├── tui/                    # Ratatui 状态、事件、视图和动作
│   └── error.rs                # 稳定错误码
├── schemas/                    # JSON Schema，与 Rust 类型同步
├── pipelines/
│   └── short-drama.yaml
├── adapters/
│   └── minimax-h3-comfy.yaml
├── workflows/                  # 导出的 ComfyUI API workflow
├── skills/
│   └── screenwriter/
│       ├── SKILL.md             # 外部 Agent 的入口提示和执行规则
│       └── examples/            # 合法 / 非法轻量 ScriptBundle
├── tests/
│   ├── fixtures/
│   ├── mock_comfy/
│   └── fault_injection/
└── examples/                   # 不含大媒体的示例合同；真实项目根在源码仓库外
```

`main.rs` 必须保持很薄。领域逻辑放在 `lib.rs` 导出的模块里，使 CLI、worker、TUI 和测试调用同一实现。

## 5. 依赖选择

| 目标 | Rust 库 / 外部工具 | 说明 |
| --- | --- | --- |
| 异步运行时 | Tokio | worker、IPC、HTTP、WebSocket 和子进程 |
| CLI | Clap | 稳定子命令和机器可读参数 |
| TUI | Ratatui + Crossterm | Linux / SSH 终端控制台 |
| 序列化 | Serde、serde_json、serde-yaml-ng | 项目合同、状态、manifest 和 adapter 配置；不采用已归档的 serde_yaml |
| JSON Schema | Schemars | 从 Rust `ScriptBundle` 类型生成 schema，测试阻止 checked-in schema 漂移 |
| 错误路径 | serde_path_to_error | 把 Agent 合同反序列化错误定位到 JSON Pointer |
| HTTP | Reqwest | ComfyUI API 和输出下载 |
| WebSocket | tokio-tungstenite | ComfyUI 实时进度，可断线重连 |
| ID | ULID | request、command、event 和 review run；可排序但不承载业务语义 |
| Hash | sha2 | workflow、模型、输入和合同指纹 |
| 文件锁 | fs4 | worker 和项目 advisory lock；不采用已停止维护的 fs2 |
| 项目归档 | tar | 标准 TAR 容器；SparkStage manifest 单独记录版本、文件大小和 SHA-256，不依赖外部 `tar` 命令 |
| 日志 | tracing、tracing-subscriber | 结构化运行日志 |
| 错误 | thiserror | 内部错误类型到稳定错误码的映射 |
| 测试 | tempfile、Ratatui TestBackend、cargo-llvm-cov | 临时项目、故障注入、TUI 快照和覆盖率门禁；CI 另跑 cargo-audit / cargo-deny |
| 媒体 | ffmpeg、ffprobe | 独立进程调用，不通过 shell 拼字符串 |

工程锁定 rustc 1.98.0 与 edition 2024，并提交 `rust-toolchain.toml` 和 `Cargo.lock`。crate 使用兼容该编译器、能在 DGX Spark aarch64 编译的版本；本机 macOS 通过不等于 DGX 兼容，DGX 烟测结果必须单独记录。

## 6. 文件与运行目录

### 6.1 机器级目录

Linux 默认遵循 XDG 目录，也允许配置覆盖；未设置 XDG 变量时使用对应的标准用户目录：

```text
$XDG_CONFIG_HOME/sparkstage/
└── config.yaml                 # 项目根、ComfyUI 地址、播放器等

$XDG_DATA_HOME/sparkstage/
├── notifications.json          # 可选的本机里程碑 hook 配置
├── runtime/
│   ├── worker.lock
│   ├── worker.sock
│   ├── queue.json
│   └── commands.jsonl          # command id 去重和提交结果
├── projects/                   # 默认 project root
└── benchmarks/h3/              # 机器级 benchmark 原始产物

$XDG_CACHE_HOME/sparkstage/
└── probes/                     # 可重建的能力与媒体探针缓存
```

机器级目录不保存项目视频、参考图或完整提示词。默认项目根是 `$XDG_DATA_HOME/sparkstage/projects/`，可显式改到其它数据盘，但不能默认为源码仓库。`queue.json` 只保存 project id、project path、job id、优先级和资源类别；`commands.jsonl` 只保存 command id、目标 revision、结果码和必要引用，不复制命令中的 prompt。

### 6.2 项目级目录

项目目录结构以 `product.md` 为准。技术上增加以下不变量：

- `script/brief.md` 保存用户原始输入；Agent 的改稿不得覆盖它。
- `script/shots.json` 是拍摄合同，不包含运行状态和 seed。
- `script/authoring.json` 冻结 skill / schema 版本、brief hash、bundle hash，以及 Agent host / model 在可获得时提供的信息；不保存外部 Agent 密钥或完整会话。
- `state.json` 是当前项目状态快照，带单调递增的 `revision`。
- `jobs/<job-id>.json` 是可变的逻辑 job journal，内部保存一次或多次 submission attempt。
- `raw/<shot>/<take-id>.json` 在输出验证后冻结，生成字段不再修改。
- `refs/<character|location>/<subject-id>/<reference-id>.<ext>` 保存不可变参考文件；`state.json.references` 冻结 SHA-256、字节数、原始文件名和替换链。
- `builds/<build-id>/subtitles.{srt,vtt}` 是 recipe 冻结 cue 的确定性派生物；交付副本位于对应 review/final 视频旁。
- `decisions.jsonl` 记录人工或策略审批，`events.jsonl` 记录机器事件。
- 临时文件与目标文件必须在同一文件系统，保证 rename 原子性。
- worker 在执行变更命令、开始 job 和回收输出前重新读取合同 hash；若外部编辑发生在生成途中，结果仍落成可追溯 take，但立即标记 stale，不能自动获批。

## 7. 标识符与哈希

| 类型 | 示例 | 规则 |
| --- | --- | --- |
| Project ID | `rain-apartment` | 小写 slug，创建后不变 |
| Shot ID | `S06` | 项目内唯一，显示顺序另存，不从 ID 猜顺序 |
| Take ID | `S06-T002` | 项目内唯一，不能只用整数引用 |
| Job ID | `JOB-01J...` | 一次逻辑生成任务，可包含多次明确记录的 submission attempt |
| Request ID | `01J...` | 每次 submission attempt 投递前创建，用于对账与幂等 |
| Command ID | `01J...` | 每次变更命令唯一，客户端重试复用 |
| Backend Job ID | ComfyUI prompt id | 后端确认接收后绑定 |
| Build ID | `BLD-01J...` | 一次 draft / trailer / final 构建；不与 benchmark 的 B01–B06 混名 |
| Reference ID | `REF-<command-hash>` | 一次不可变参考导入；替换生成新 ID，旧 ID 保留 |

合同哈希不能直接依赖用户文件的空格和键顺序。实现先把已验证的 JSON 反序列化为强类型结构，再按稳定字段顺序输出 canonical JSON，最后做 SHA-256。参考文件和 workflow 对原始字节做流式 SHA-256。

## 8. 核心数据模型

### 8.1 BibleIndex 与 ShotContract

`bible/index.json` 为人物和地点提供稳定 ID，并记录对应描述文件与参考素材。`ShotContract.characters[]` 必须列出所有出镜人物，`ShotContract.location` 必须引用主地点；依赖关系不从 prompt 文本猜测。

```json
{
  "schema_version": "1.0",
  "characters": {
    "zhao": {
      "source": "bible/characters/zhao.md",
      "references": ["refs/characters/zhao-approved.png"]
    },
    "lin": {
      "source": "bible/characters/lin.md",
      "references": ["refs/characters/lin-approved.png"]
    }
  },
  "locations": {
    "apartment-living-room": {
      "source": "bible/locations/apartment-living-room.md",
      "references": ["refs/locations/apartment-living-room.png"]
    }
  },
  "style_source": "bible/style.md"
}
```

人物和地点使用独立源文件，使单个实体的 hash 变化只传播到引用该 ID 的镜头。index、source 或 reference 缺失时项目 preflight 失败。

schema 执行跨字段校验：

- `characters[]` 和 `location` 必须存在于 bible index。
- `dialogue[].who`、`camera.screen_direction` 及 continuity 中的角色键必须属于 `characters[]`。
- silent character 也必须列入 `characters[]`。
- `i2v`、`flf2v`、`r2v` 的 conditioning 按 operation 校验。
- 对白估算时长不得超过镜头时长减去头尾呼吸预算。

对白估算使用 pipeline profile 中可配置的中文字符速率、标点停顿和头尾余量。它是编剧阶段的确定性容量检查，不替代对 H3 实际语速和台词正确性的人工 / ASR 核对。

### 8.2 CreativeBrief、ScriptBundle 与 AuthoringReceipt

外部 Agent 不直接创建项目内任意路径。`skills/screenwriter/` 要求它根据 `CreativeBrief` 返回一个有版本的 `ScriptBundle`：项目摘要、人物、地点、风格、故事、shot list 和 typed shots。`sparkstage script validate` 对 bundle 完成 JSON Schema 与跨字段校验；`script apply` 由 worker 重新校验后，用确定性 renderer 生成 `PROJECT.md`、`bible/**` 和 `script/**`。

编剧入口采用以下顺序：

1. `project new --brief-file` 保存不可变 brief 和 resolved pipeline。
2. 外部 Agent 读取编剧 skill、目标 schema 和允许的文本上下文；它内部如何生成或改稿不属于 SparkStage runtime。
3. `script validate bundle.json --json` 执行内容输入规则、ID 引用、镜头总时长、对白时长、角色 / 地点和 conditioning 校验，并返回稳定错误码与 JSON Pointer。
4. Agent 根据校验结果修改 bundle；Rust 不发起模型调用，也不实现自动 prompt repair loop。
5. `script apply bundle.json` 由 worker 重验、写入 staging、生成 `AuthoringReceipt` 并创建 `script_bundle` approval。
6. 用户批准后通过单个 worker 命令原子提升为当前合同；拒绝或重新导入时保留旧草稿和原始 brief。

Bundle 不能包含 ComfyUI node id、scheduler、步数或 workflow JSON。具体 pipeline 是产品配置，Agent 只为它填充语义合同。skill 可以有适配 Codex、Claude Code 或其它 Agent host 的薄入口，但它们必须指向同一 schema，不能各自发明字段。

`script evaluate --suite` 在 validate 之上提供独立的评测层。suite 为每个外部采集 bundle 冻结 expected validity、结构摘要、issue code、Agent host/model、质量样本标记与 repair count；输出稳定 JSON，汇总首次通过率、修复次数、issue 分布以及逐 Agent/model 指标。评测命令不调用 LLM，也不修改项目，checked-in fixture 的通过只证明样本符合冻结期望，不能外推成模型质量结论。

### 8.3 ProjectState

```json
{
  "schema_version": "1.0",
  "revision": 42,
  "project_id": "rain-apartment",
  "project_stage": "shooting",
  "project_outcome": "needs_review",
  "work_mode": "director",
  "quality_target": "playable",
  "pending_approvals": [
    {
      "approval_id": "APR-01J...",
      "kind": "candidate_selection",
      "shot_id": "S06",
      "take_ids": ["S06-T001", "S06-T002"],
      "blocking": true
    },
    {
      "approval_id": "APR-01K...",
      "kind": "budget_overrun",
      "shot_id": "S09",
      "take_ids": [],
      "blocking": true
    }
  ],
  "shots": {
    "S06": {
      "stage": "candidates_ready",
      "active_job_id": null,
      "selected_candidate_take_id": null,
      "approved_take_id": null,
      "take_ids": ["S06-T001", "S06-T002"],
      "fail_codes": [],
      "stale": false
    }
  }
}
```

`revision` 每次成功变更加一。`pending_approvals[]` 是待处理审批的唯一集合，允许不同镜头和预算审批同时存在；每个 approval 有稳定 ID。`project_outcome=needs_review` 是由存在 blocking approval 派生并在写入时校验的摘要，不单独制造另一份审批真相。

TUI 或 CLI 提交审批时携带 approval id 和它读取到的 `expected_revision`；若状态已变化，worker 返回 `REVISION_CONFLICT`，客户端刷新后让用户重新确认，防止在旧画面上批准错误 take。

### 8.3.1 BudgetContract

预算合同持久化在 `ProjectState.budget`，包含独立 `contract_revision`、总墙钟、每镜 audition/final take 上限、最低剩余磁盘、云费用开关、超限策略和估算 profile。当前默认 profile 的 source 必须显示为 `unmeasured_default_v1`：4 小时、每镜 3 个 audition / 2 个 final、5 GiB 磁盘硬线，audition/final 暂按每视频秒 30/120 墙钟秒和 4/12 MiB。它们是保守占位值，不是 DGX 或 H3 benchmark。

排队前按当前合同、已产出 take 和计划镜头重新计算增量。时间或 take 维度超限会创建稳定 ID 的 blocking `budget_overrun` approval；批准只授予对应合同 revision、镜头和维度，修改预算合同会递增 revision 并清除旧授权。磁盘低于 hard floor 直接返回 `DISK_BUDGET_EXCEEDED`，任何 approval 都不能绕过。`budget status/default/apply` 与 TUI 读取同一份快照。

### 8.4 JobJournal

```text
logical job: queued → active → completed
                   ├→ blocked
                   ├→ failed
                   └→ cancelled

attempt: prepared → submitting → submitted → running → backend_succeeded
                         │             │              ├→ output_validated → completed
                         │             │              └→ output_invalid
                         │             ├→ backend_failed → retry_wait → prepared (new attempt)
                         │             └→ cancelled
                         └→ submission_unknown
```

job journal 至少包含：job id、command id、project / shot / reserved take id、operation、resolved prompt、seed、profile 快照、输入 hash、adapter 指纹、当前 job stage，以及 `attempts[]`。每个 attempt 保存独立 request id、backend job id、状态、时间戳、错误和输出定位信息。

`submitting` 表示 POST 已经准备执行但尚未安全记录响应。worker 在这个状态退出后，恢复时必须先对账，不能直接回到 `prepared`。只有 `/history` 明确返回执行错误或确认原任务不存在，才进入 `backend_failed` 或允许新 attempt；自动重试先经过 `retry_wait`、预算和次数检查，并为新 attempt 创建新的 request id。

### 8.5 Take

Take 是已发生生成的不可变证据。只有输出通过路径检查、文件落盘和 ffprobe 硬检查后才创建或冻结 take metadata。审片结论通过新的 review run 追加，当前候选选择和最终批准只写进 `state.json` 与 `decisions.jsonl`。

### 8.6 QueueState

全局 `queue.json` 至少包含：schema、revision、paused、running job 引用和 pending job 引用。它不复制完整 shot 或 prompt；worker 从项目 job journal 读取执行输入。

### 8.7 BuildRecord

Build 使用独立于 GPU camera queue 的单执行器，状态为 `queued -> running -> needs_review -> approved`；执行、媒体硬检查或恢复失败进入 `failed`。`queued` 表示配方与状态已经持久化但 ffmpeg 尚未取走任务，`running` 只在执行线程发出 started 事件后写入，因此 TUI 能区分等待与实际封装。

每条记录保存原始 command id、recipe 路径、输出路径、警告和 stale 标记。不可变 recipe 固化合同 hash、源 revision，以及每个输入 take 的 profile、input hash、adapter / workflow / model 指纹、seed、引用 subject 列表和 active-reference fingerprint。recipe 还可冻结对白 source hash、完整 subtitle cue、build/delivery 的 SRT/VTT 路径与内容 hash。`edit build --kind draft --shots S04-S07,S10` 在 CLI 展开为显式 shot ID，worker 校验重复和未知 ID，再按合同顺序写入 recipe；省略 `--shots` 才表示全片。局部范围只允许 draft，final 与 trailer 必须覆盖完整合同，避免片段通过终片审批后把项目错误标为 done。Build 配方要求每个 take 已有安全的项目内首帧路径，缺失或越界时必须在启动 FFmpeg 前失败。成片通过 ffprobe 与黑帧、静帧、静音硬检查后，才发布交付副本，并生成 `builds/<build-id>/subtitles.{srt,vtt}`、`builds/<build-id>/contact-sheet.jpg`、delivery 字幕、`review/contact-sheet.jpg` 和包含完整配方血缘的 `review-report.json`；随后进入人工 build review，final build 仍必须经过 `final_visual_review` 才能升为 `playable + done`。当 take 决策、引用 fingerprint 或合同变化使 build stale 时，对应的待审批项同时撤销，旧产物仍保留但不能获批。worker 重启会恢复 `queued` 与 `running` build；缺失、损坏或身份不匹配的 recipe 只将对应 build 标为 `failed`，不能阻止其它项目启动。

## 9. 唯一写入与落盘协议

### 9.1 单 worker

- worker 启动时独占 `worker.lock`，第二个 worker 直接失败。
- 每个活跃项目再持有 project lock，防止迁移或外部工具同时写。
- CLI、TUI 和审片页不直接写 JSON。
- Agent 只在临时位置生成 ScriptBundle；active contract 与运行状态都必须经 worker 命令写入。

### 9.2 原子快照

JSON 快照采用同目录临时文件协议：

1. 序列化并校验完整新状态。
2. 写入唯一临时文件。
3. flush 并 `fsync` 临时文件。
4. rename 覆盖目标文件。
5. `fsync` 父目录。

读取方永远只看到旧快照或新快照。未知 schema、损坏 JSON 或 revision 倒退都禁止继续写。

### 9.3 跨文件命令与决策日志

没有数据库时不能假装多个文件天然事务化。每个变更命令按以下次序执行：

1. 以 `command_id` 在 worker command journal 写入 `prepared`；需要外部副作用时先写 job/attempt 恢复依据。
2. 对将随状态一起生效的 decision batch，先逐条追加 `phase=prepared`，每条带稳定 event id、command id 和目标 revision；history 不展示 prepared 记录。
3. 原子写项目状态，并把 `last_command_id` 设为当前 command；需要时再原子写 queue 或 cleanup plan 的 operation 状态。
4. 为同一批 event 追加 `phase=committed`，最后把 worker command journal 标为 `committed` 并返回结果。

worker 重启或下一次项目写入前扫描未完成记录：若 `state.last_command_id` 与 prepared command 匹配，则幂等补齐 committed；否则隐藏并放弃该 prepared decision，不能从 JSONL 反推状态。读取端按 event id 去重，旧版没有 phase 的 decision 兼容为 committed。cleanup plan 在移动文件前写 `applying/restoring` 与 active operation，恢复时同时核对源路径和 trash 路径，因此在 rename 中途退出也能继续而不重复移动。

worker 内部采用单 actor 提交状态：HTTP、WebSocket、ffmpeg 和审片任务可以异步执行，但它们只能把结果消息发回 actor，由 actor 串行更新 job、project state 和 queue。模块持有文件路径不等于拥有写权限。

### 9.4 离线校验、归档与 schema 迁移

`project verify/export/verify-archive/import/migrate` 是显式维护命令，不进入运行态 command actor。`verify` 和 `export` 持有 project lock；`import` 持有 projects 目录级锁并使用隐藏暂存容器；`migrate --apply` 持有 project lock。worker 的正常变更也使用同一 project lock，因此维护命令与运行态写入不会并发落盘。

归档格式是标准 TAR，第一项固定为 `archive-manifest.json`，其余项只能是 `project/<safe-relative-path>` 普通文件。manifest 按路径排序，记录 archive schema、project schema、项目 ID、字节数和逐文件 SHA-256；`project.lock` 不归档，symlink、特殊文件、重复/额外路径、非 UTF-8 路径和目标位于源项目内部都拒绝。验证还要解析 project/state、核对 brief hash、每个 contract bundle hash 以及 state 中每个参考文件的大小/SHA-256，不能只验证 TAR header。导入完整执行两遍验证并在暂存项目按真实 ID 打开成功后才 rename 发布；已存在目标永不覆盖。

迁移默认 dry-run，报告源/目标 schema、具体变更和将使用的 backup 路径。当前只支持项目与 state 分别处于 `0.9` 或 `1.0` 的恢复性升级到 `1.0`；`--apply` 在任何项目文件修改前复制原始 `project.json`、`state.json` 和 plan 到 `backups/migrations/<migration-id>/`，再分别原子替换。未知 schema 只返回不可应用计划；不推断或迁移人工审批语义。

## 10. IPC 与命令协议

MVP 只开放 Unix domain socket，socket 权限限制为当前用户。协议使用带长度前缀的 JSON frame，并限制单条消息大小；不通过 socket 传视频或图片。

请求统一包含：

```json
{
  "protocol_version": "1.0",
  "command_id": "01J...",
  "command": "shots.approve",
  "project": "/data/projects/rain-apartment",
  "expected_revision": 42,
  "args": {
    "approval_id": "APR-01J...",
    "shot_id": "S06",
    "take_id": "S06-T002"
  }
}
```

响应统一包含 `ok`、`command_id`、最新 revision、data 或稳定 error：

```json
{
  "protocol_version": "1.0",
  "ok": false,
  "command_id": "01J...",
  "revision": 43,
  "data": null,
  "error": {
    "code": "REVISION_CONFLICT",
    "message": "项目状态已经变化，请刷新后重新确认。",
    "retryable": true
  }
}
```

同一 `command_id` 重试必须返回第一次提交的结果，不能重复排队或重复审批。CLI 的 `--json` 直接输出这一稳定 envelope；普通模式再渲染为中文。

批量审批使用一个 command 携带多个 approval id 和各自选择，worker 在同一 expected revision 下全部校验后一次提交；任一项失效则整批拒绝，避免半批成功造成用户干预计数和实际状态不一致。

## 11. Worker 生命周期

启动顺序固定为：

1. 取得 worker lock，加载配置并检查目录权限。
2. 读取全局 queue，拒绝未知 schema。
3. 扫描未完成 command 和 job journal，先恢复内部一致性。
4. 对有 backend job id 的任务查询 `/history` 和队列状态。
5. 对每个 attempt 的 `submission_unknown` 尝试 request 标记对账；不能确认则保持阻断。
6. 将 `/history` 的明确执行错误落为 `backend_failed`；验证成功任务的输出并创建 take。
7. 开启 IPC、调度循环和 ComfyUI WebSocket 监听。

关闭时先停止接收新 GPU job，完成状态落盘，释放 socket 与锁。普通停止不杀当前 ComfyUI 任务；明确取消才调用 adapter 的取消能力。

运行时分两种所有权：

- `managed`：SparkStage 启动的 ComfyUI，可以按配置限次重启。
- `attached`：用户或其它服务启动的 ComfyUI，SparkStage 只探活和等待，不擅自杀进程。

## 12. ComfyUI Adapter

### 12.1 责任

adapter 只负责把产品语义翻译为具体 workflow：

- preflight 与 capability report
- 上传或定位输入素材
- 按 binding 修改结构化 API workflow JSON
- 提交 prompt，记录 backend job id
- 监听 WebSocket 进度并在断线后重连
- 通过 `/history` 判断终态
- 解析输出定位并安全收取媒体
- 可选取消和运行时管理

领域层不能知道 ComfyUI node id。

### 12.2 Trait 边界

```rust
trait CameraAdapter {
    async fn preflight(&self) -> Result<CapabilityReport>;
    async fn prepare(&self, request: GenerationRequest) -> Result<PreparedJob>;
    async fn submit(&self, attempt: &PreparedAttempt) -> Result<BackendJobId>;
    async fn reconcile(&self, job: &JobJournal) -> Result<BackendState>;
    async fn fetch_outputs(&self, job: &JobJournal) -> Result<Vec<OutputArtifact>>;
    async fn cancel(&self, job: &JobJournal) -> Result<CancelOutcome>;
}
```

具体签名可随 Rust 实现调整，但领域输入、后端 job id 和输出产物不能揉成一个无类型 JSON。

### 12.3 Binding 配置

```yaml
schema_version: "1.0"
adapter: minimax-h3-comfy
endpoint: http://127.0.0.1:8188
allow_remote: false
allow_global_interrupt: false
workflow: workflows/minimax-h3-api.json
output_node: "120"
bindings:
  prompt: { node: "45", input: text }
  seed: { node: "78", input: noise_seed }
  output_prefix: { node: "120", input: filename_prefix }
optional_bindings:
  first_frame: { node: "31", input: image }
  last_frame: { node: "32", input: image }
```

preflight 校验 workflow 文件 hash、节点存在、input 名、输出节点和模型文件。可选 binding 缺失会让相关 capability 变成 `unsupported` 或 `degraded`，不会影响已经验证的 T2V。

ComfyUI V3 的 autogrow 输入在 API prompt 中使用点路径，例如 H3 的 `ref_images.ref_image_0`；服务端执行前会把这些路径还原为节点需要的嵌套 map。adapter 不得把参考图 links 写成单个 `ref_images: { ... }` 值，否则 ComfyUI 会忽略动态输入。H3 的 `ref_videos` 同样是 autogrow 的 `IMAGE` 帧流，不是可直接填入文件名的字符串；在完成视频加载、帧提取和真实烟测前保持该能力未配置。

首次能力认证使用显式 `shots smoke-test --accept-unverified`，它仍经过项目审批、预算、持久 job、worker GPU 队列、adapter、媒体检查和 take 血缘，只对本次单 take 跳过“已经 verified”这一循环前置条件。普通 audition / final 不接受此豁免；smoke-test 成功也不会自动修改配置，必须先人工核对 job、媒体探针和输出，再记录对应 `verified_operations`。

### 12.4 安全投递协议

1. 首次执行创建 job id 和 reserved take id；每次投递创建新的 attempt request id 与完整 PreparedAttempt。
2. 把本 attempt 的 request id 注入输出前缀或 workflow 可回查 metadata。
3. 把 attempt 追加到 job journal 并落为 `prepared`，再更新为 `submitting`。
4. POST ComfyUI `/prompt`。
5. 收到 prompt id 后，立刻绑定为 `submitted` 并落盘。
6. WebSocket 更新实时进度；断线不改变 job 真相。
7. `/history/<prompt-id>` 显示成功后解析输出；明确节点错误则写入 `backend_failed`，保存错误摘要并按策略进入 `retry_wait` 或 job `failed`。
8. 下载到项目临时文件，完成路径、大小和媒体检查后原子改名。

步骤 4 与 5 之间存在无法靠普通 HTTP 消除的崩溃窗口。若 workflow 能按 attempt request id 搜索队列、历史或输出前缀，worker 自动对账；否则保持 `SUBMISSION_UNKNOWN` 并请求人工确认。系统宁可停住，也不在不知道原任务是否存在时创建下一 attempt。

### 12.5 输出安全

- 只接受 `/history` 返回并属于已声明 output node 的产物。
- 对 filename、subfolder 和 type 做结构化解析与 URL 编码，不拼接未经验证的路径。
- 下载或复制后确认文件仍位于项目 staging 目录内，拒绝 `..`、绝对路径和符号链接逃逸。
- 先 ffprobe，再移动到 `raw/`；无效输出保留 job 和诊断，不创建可选 take。

## 13. 队列与资源调度

任务声明资源类别：

| 类别 | 示例 | 调度规则 |
| --- | --- | --- |
| `gpu_exclusive` | H3 视频生成、重型增强 | 全机同时一个 |
| `gpu_benchmark` | H3 基线、attention、compile、cache、量化实验 | 与 `gpu_exclusive` 共用同一独占锁，不能直连 ComfyUI 绕过队列 |
| `gpu_aux` | 可能使用 GPU 的审片模型 | 不与视频生成并发，除非实测允许 |
| `cpu_media` | ffprobe、轻量抽帧、报告 | 有界并发 |
| `io_heavy` | 大文件复制、最终封装 | 避免和输出写盘高峰重叠 |

优先级为 interactive、normal、background，同级 FIFO。正在运行的 job 不抢占。默认最多连续运行 3 个 interactive job；之后若 normal 已等待则让出一次。background 通过等待时间提升有效优先级，避免永久饥饿。所有数字放进配置并由基准测试校准。

暂停只阻止新 job 开始。pending job 可以直接取消；running job 只有在 attempt 已持久化 backend job id、目标确为全局队列中唯一的 GPU job，并且 adapter 明确配置 `allow_global_interrupt: true` 时才调用 ComfyUI `/interrupt`。后端返回成功前不得把本地 job 标为 cancelled；返回后先持久化 `attempt=cancelled`、`job=cancelled` 与 shot 状态，再通知相机执行线程退出 WebSocket / reconcile 等待。未到可中断阶段返回 `JOB_CANCEL_NOT_READY`，未显式启用返回 `JOB_CANCEL_UNSUPPORTED`，HTTP 失败返回可重试的 `JOB_CANCEL_FAILED`，三者都保持原运行状态。

`sparkstage benchmark h3` 是 worker 的受控命令，不是第二套 ComfyUI 客户端。它先等待当前 GPU job 到达安全边界，再取得 benchmark reservation；未获锁时不能清理、终止或覆盖其它项目任务。benchmark 调用生产 adapter 的 prepare / submit / reconcile / fetch 路径，只在外围增加 profiler、遥测和实验报告。

当前无需 DGX 的 P0 只实现 `benchmark h3 init/record/show`：`init` 冻结 adapter、workflow、profile 与环境指纹，`record` 追加已有生产 job 的原始样本和 evidence，`show` 读取不可变记录。这三个命令不访问 GPU，也不能把 run 标为 `verified`。后续发起真实 H3 实验的执行命令仍按上一段进入 worker 与 `gpu_benchmark` reservation；`record` 不是第二套投递客户端。

## 14. Pipeline 与 Profile

Pipeline 描述制片语义，adapter profile 描述模型参数，两者分开：

```yaml
schema_version: "1.0"
id: short-drama
stages: [project, script, shooting, review, build]
defaults:
  duration_seconds: 5
  width: 960
  height: 544
  fps: 24
profiles:
  audition: minimax-h3-comfy/audition
  final: minimax-h3-comfy/final
authoring_skill: screenwriter/v1
dialogue_budget_profile: mandarin-h3-baseline
review_rules:
  hard: [decode, duration, audio_presence, black_frame]
approval_gates: [project_test, candidate_selection, mvp_final_visual_review]
```

外部 Agent 的具体模型不进入 pipeline；这里只固定 `authoring_skill` 与 bundle schema 版本。视频模型的步数、scheduler、attention、量化和节点 id 只出现在 adapter profile。优化 profile 的准入、对照和回退遵循 `optimization.md`，不能由 TUI 临时勾选一堆组合后覆盖可追溯配置。

## 15. 媒体与成片

### 15.1 Take 硬检查

每个输出至少检查：

- 容器与视频流可解码
- 分辨率、fps、帧数和时长符合 resolved profile
- 要求原生音频时存在音轨
- 音视频时长偏差在 pipeline 容差内
- 文件非零、无长黑帧、无长静帧和异常静音

静帧使用 FFmpeg `freezedetect`（最短 1.5 秒）检测并汇总闭合区间与延伸到 EOF 的开放区间。`media_check_profiles` 按生成 profile 配置允许的总冻结占比；未配置时默认 30%，当前 H3 adapter 的 baseline/audition 为 30%、final 为 20%。配置必须落在 0.0–1.0，且名称必须对应已有生成 profile，adapter preflight 在投递前拒绝非法值。其它阈值继续按 H3 实测和拼片需求校准。

### 15.2 ffmpeg 调用

- 使用 `tokio::process::Command` 传参数数组，不拼 shell 命令。
- 每次运行记录工具版本、argv、输入 hash、退出码和 stderr 摘要。
- 输出先写 staging，探针通过后原子移动。
- 成片 recipe 保存镜头顺序、take id、trim、音频处理、字幕和编码参数。
- build 不修改 raw；任何增强或转码都是带 parent 的派生产物。

### 15.3 音频

MVP 的角色对白由 H3 原生音频生成，不接独立 TTS。编剧阶段先用 dialogue budget profile 做确定性时长估算；生成后统一采样参数和响度，再由人确认台词与可懂度。补帧或变速后按原时长重新对齐音轨。FunASR、VAD、SyncNet 和更完整的声音处理按 `optimization.md` 分阶段接入，它们是核对器，不是对白生成器。

### 15.4 对白字幕

字幕是 build 的确定性后期投影，不烧进 raw。每个纳入 build 的镜头以裁切后时长形成时间段，同镜头多句对白等分该时间段；SRT 使用逗号毫秒，WebVTT 使用点号毫秒并以 shot ID 作 cue identifier。局部 draft 只遍历显式选择的镜头且 offset 从零开始，trailer 与视频相同地把每镜裁到最多两秒。无对白时不创建空字幕文件。

规划阶段在 `BuildRecipe` 冻结 normalized speaker/text、开始/结束毫秒、对白 source hash、四个安全相对路径和 SRT/VTT 内容 hash。执行阶段重新渲染并先核对 hash，再原子写 `builds/<id>/subtitles.srt|vtt`，最后复制到 draft/trailer/final 视频旁；cue 或 recipe 被篡改时 build 失败，不发布不匹配的字幕。

## 16. 审片设计

### 16.1 MVP

MVP 自动化只执行有确定结果的硬门：文件、流、规格、时长、音轨、明显黑帧 / 静帧、输入侧内容规则和预算。它不能仅凭提示词确认输出没有幼态、脏字、人体错误或叙事偏离。多个候选都通过时，若没有唯一的确定性选择依据就必须人工选择。

即使所有机器硬门通过，完整终片候选仍写为 `draft_cut + needs_review`，并创建一个 blocking `final_visual_review` approval。用户至少完整看过一次画面、确认没有输出侧内容安全阻断且主要情节可理解后，worker 才允许把质量升为 `playable`、结果改为 `done`。这条 gate 在 MVP 快速模式中也不能关闭。

### 16.2 v1 检查器

Qwen3-VL、人脸 embedding、OpenCV、PySceneDetect、LPIPS / DISTS、FunASR 和运动分析以独立 checker 接入。每个 checker 返回：版本、输入帧、指标、阈值、失败码、证据帧和耗时。检查器失败不能把 take 误判为通过。

ReviewRun 只追加，不覆盖旧结果。改变 checker 版本或阈值会产生新 run；当前是否接受仍由项目状态和 decision 决定。

## 17. Stale 与依赖

stale 的业务传播规则以 `product.md` 的表格为唯一规范，本节只定义实现来源。MVP 不存一份可漂移的手工依赖图：worker 从 `bible/index.json`、`ShotContract.characters[]`、`ShotContract.location`、conditioning 引用、active reference、build recipe 和各自 hash 重建依赖。角色无台词也不会漏掉，地点依赖也不从 prompt 猜测。

`refs impact` 是参考变更的 dry-run：按 subject ID 枚举合同中依赖镜头、其中仍有效的 take，以及 recipe 输入命中这些镜头的非 stale build。真正导入/替换要求相同 revision 下重新计算，并在存在生产产物时要求 `--accept-impact`；任何受影响 camera job 或任意 queued/running build 存在时拒绝变更。新参考进入 active fingerprint 和后续 generation input hash；旧参考文件及其替换链永久保留。只把依赖镜头的有效 take/build 标 stale，不重复处理已经 stale 的产物。

合同替换分别计算 generation dependency 和 post-production dependency。prompt、规格、operation、camera、conditioning、continuity、generation plan 或所引用 bible 变化会失效 raw take；`dialogue` 或其它仅影响 build 的镜头字段只失效包含该镜头的 build，因此修改字幕文本不要求重拍。旧文件不删除，批准记录不迁移到新 take；实现测试直接覆盖这些分支。

## 18. Ratatui TUI

### 18.1 定位

TUI 是同一命令面的高密度控制台，适合本机终端和 SSH。它不读取半写文件，不直接操作 ComfyUI，也不持有自己的业务状态。

### 18.2 页面

| 页面 | 内容 | 主要动作 |
| --- | --- | --- |
| Projects | 项目列表、阶段、结果、revision 和暂停状态 | 切换项目、pause、resume |
| Dashboard | 项目阶段、结果、GPU 当前任务、预算、待审批和最近失败 | 进入待处理项 |
| Review | 当前 blocking approvals 与多镜候选 | 批量选择、批准、显式接受 warning |
| Shots | 镜头阶段、风险、候选数、批准 take、stale | audition、direct render、retry |
| Takes | 当前镜头候选、profile、硬检查、分数与警告 | select、approve、reject、preview |
| Queue | running / pending、优先级、ETA 和资源类别 | pause、resume、cancel |
| Builds | draft、trailer、final 与 recipe 状态 | build、open、rebuild |
| Storage | 总占用、回收区和可回收候选 | status、plan、apply、restore |
| History | committed decision、command 和时间 | 查看最近决策，不从日志猜审批 |
| Diagnostics | preflight、adapter capability、失败码和日志摘要 | retry probe、打开日志 |

宽终端使用列表 + 详情双栏；窄终端退化为单栏，不截断 ID 和失败码。颜色只作辅助，状态同时使用文字或符号表达。

### 18.3 数据流

- 初次进入通过 IPC 获取完整 snapshot 和 revision。
- worker 仅在成功写命令、camera attempt / queue 状态迁移或 build 状态迁移后，通过本机长连接推送 project revision 与 queue revision；空闲循环不扫描项目文件。
- 订阅断线后短退避重连；独立的 1 秒 snapshot 轮询保留为断线和漏通知兜底。
- TUI 收到新 revision 后拉取新快照，不自行合并领域状态。
- 所有变更动作携带 command id 和 expected revision。
- `REVISION_CONFLICT` 时刷新并保持用户当前位置，不自动重复审批。

### 18.4 终端生命周期

实现一个 RAII `TerminalGuard`：进入 alternate screen 和 raw mode；正常退出、错误、panic、SIGINT / SIGTERM 时都尽力恢复光标、raw mode 和原屏幕。panic hook 必须先恢复终端再输出错误。事件读取和 worker 更新通过 channel 汇合，渲染有固定上限，不因日志洪水满速重绘。

### 18.5 预览

MVP 不依赖 Kitty / Sixel 等终端图片协议。`preview` 向 worker 请求已验证媒体路径，再由 TUI 进程以参数数组调用用户配置的播放器；后台 worker 不负责猜测 DISPLAY 或 SSH 显示环境。需要占用当前终端的播放器启动前，TUI 暂时恢复 normal screen 和 cooked mode，播放器退出后再安全进入 TUI。没有播放器时显示路径和联系表。播放器失败只影响预览，不改变 take 或 job 状态。

当前锁定 Ratatui 0.30.2 与 Crossterm 0.29；macOS 无 GPU 测试已通过，DGX Spark aarch64 的终端、SSH 和播放器行为仍需实机烟测，不能只根据官网示例推断兼容。

## 19. 错误与重试

| 类别 | 示例 | 默认处理 |
| --- | --- | --- |
| 可重试传输错误 | WebSocket 断开、临时 HTTP 失败 | 有界指数退避，不新建 request |
| 后端任务未知 | `SUBMISSION_UNKNOWN` | 对账；无法确认则人工处理 |
| 后端执行失败 | `backend_failed`、ComfyUI 节点错误 | 保存 attempt 错误；策略允许时进入 `retry_wait` 并创建新 attempt |
| 文案合同错误 | `SCRIPT_BUNDLE_INVALID`、JSON Pointer | 返回全部可定位错误；不得导入或创建 H3 job |
| 能力 / 配置错误 | `CAPABILITY_MISS`、`WORKFLOW_INVALID` | 排队前失败，修配置后再请求 |
| 资源错误 | `DISK_LOW`、worker lock 冲突 | 保持产物，等待释放资源 |
| 输出错误 | `OUTPUT_INVALID`、媒体不可解码 | 保留 journal，禁止生成 take |
| 质量错误 | `FACE_DRIFT`、`AUDIO_MISS` | 按预算和重拍策略处理 |
| 并发冲突 | `REVISION_CONFLICT` | 刷新状态，要求重新确认 |

同一 attempt 的安全传输重试复用 request id；后端已经明确接收并失败后，下一次执行必须在同一逻辑 job 下追加新 attempt 和新 request id。若用户改变 prompt、profile、conditioning 或 seed，则创建新的逻辑 job / take 候选，而不是把它伪装成原 attempt。任何重试都受项目预算和最大次数限制。

## 20. 安全边界

- ComfyUI 默认只允许 loopback 或显式批准的本机地址。
- Unix socket 使用当前用户权限，不在 MVP 开 TCP 控制端口。
- SparkStage 不托管外部 Agent 的认证或 API Key，也不向它自动上传文件。`screenwriter` skill 默认只允许读取 brief、pipeline 文本规则、schema 和已明确授权的文本合同；raw、参考图、音频、成片和其它项目不在读取范围内。
- 云 adapter 默认禁用；首次上传前生成数据清单并要求项目级批准。
- 每个第三方模型、权重、ComfyUI 节点、审片器和增强器都进入 dependency manifest，分别记录代码许可、权重许可、来源、允许用途和核验日期；`unknown` 或 `non_commercial` 不能进入商业 profile。
- 密钥不写项目、JSONL、命令行参数或日志；只从受控配置源读取。
- 所有项目相对路径 canonicalize 后必须仍在项目根内。
- 外部命令使用固定可执行文件和参数数组，用户文本不能进入 shell。
- 通知 hook 只接受绝对路径、可执行、非 symlink 的普通文件；启动前清空继承环境，不解析 shell，只以固定 argv、三个最小标识环境变量和 stdin JSON 传递事件。
- 下载设置大小上限、超时和允许的媒体类型。
- 日志对 prompt、人物素材路径和远端响应做最小化记录，不打印完整环境变量。

## 21. 可观测性

每个日志和事件携带 project id、command id、request id、shot id、take id 和 backend job id 中适用的字段。关键 span 包括：

- command validation / commit
- queue wait
- adapter prepare / upload / submit / reconcile / download
- model load、text、DiT、VAE、audio 和 mux（能观测时）
- media probe / review / build

普通日志供诊断，`events.jsonl` 供产品界面，benchmark 产物遵循 `optimization.md`。不能从日志反向构造人工审批。

可选 `HookDispatcher` 只消费七类已提交的产品里程碑：`approval_required`、`take_ready`、`camera_failed`、`build_completed`、`build_failed`、`disk_blocked`、`project_completed`。worker 自动读取 data home 下的 `notifications.json`，也允许 `worker run --hook-config` 显式覆盖。事件在项目/队列状态提交后送入独立 channel，由单线程串行启动 hook；hook 慢、退出失败或 receiver 关闭只输出诊断，不回滚状态、不占用 worker actor 或 GPU executor。

## 22. 测试策略

### 22.1 不需要 GPU 的测试

- shot、state、job、take 和 IPC schema round-trip
- 每条状态迁移的允许 / 禁止矩阵
- command id 幂等与 revision conflict
- 原子写入中断、损坏 JSON 和未知 schema
- 队列公平性、暂停、取消和预算上限
- stale 传播与 build recipe
- 不可变参考导入/替换、SHA-256 篡改、影响确认、精确 take/build 失效和引用 fingerprint
- SRT/VTT cue 时间、局部 draft 范围、recipe/content hash、交付副本和 dialogue-only build stale
- bible index、silent character、location 与 dialogue cross-field 校验
- ScriptBundle 的合法输入、非法 JSON、越权字段、JSON Pointer 错误和原子提升
- 不同外部 Agent 产生的 fixture 通过同一 contract suite，并生成首次通过率、repair、issue code 与 Agent/model 聚合报告
- 本机 hook 配置、绝对可执行文件、symlink 拒绝、无 shell 参数、清空环境和 JSON stdin
- ComfyUI mock 的成功、失败、断线、history 延迟和非法输出路径
- mock adapter 在不改 `shots.json` 的前提下通过同一 contract suite；第二个真实 adapter 留到 v2 验证
- 合成媒体 fixture 的必选/可选音轨、静音、错时长、FPS、黑帧、静帧、首尾/接力帧，以及两段真实 FFmpeg build、交付副本、联系表和血缘报告；运行时预检失败时才允许跳过，开始生成夹具后不吞错误
- 项目 verify、带逐文件 SHA-256 的 TAR export、篡改拒绝、verify-before-import、禁止覆盖、schema migration dry-run 和修改前备份
- decision prepared/committed、批量原子 history、cleanup apply/restore 中断恢复
- Ratatui TestBackend 的宽 / 窄布局与关键状态快照
- panic 和退出后的终端恢复单元边界

GitHub Actions 在 Linux/macOS 上执行 fmt、严格 Clippy 和 all-target tests；Linux 安装标准 FFmpeg，使合成媒体测试走完整路径。CI 还要求每个 Rust 文件少于 900 行，超过阈值时应按职责拆分模块，而不是压缩格式。另有 Linux aarch64 cross-check、`cargo llvm-cov --fail-under-lines 70`、`cargo audit --deny warnings` 和 `cargo deny check`。当前本地无 GPU 基线为 243 个测试（240 个单元测试和 3 个 CLI 集成测试）；最近已记录的行覆盖率为 70.07%，本次变更未重跑覆盖率，仍由 CI 的 70% 门禁复核。关键纯逻辑目标为 85%+。全局覆盖率先稳定在 70%–75%，不为统一达到 90% 测试终端 raw mode、平台外壳或未接入的 DGX/H3 路径。本地随应用附带的裁剪版 FFmpeg 若缺 lavfi 源会在 fixture preflight 阶段跳过，因此本地“测试通过”不能代替 CI 的标准 FFmpeg 结果。

### 22.2 DGX Spark 集成测试

1. T2V 最小 workflow 烟测并保存 capability report。
2. worker 在运行中被终止，重启后绑定原 prompt id。
3. 在提交窗口注入退出，验证 `SUBMISSION_UNKNOWN` 不盲投。
4. WebSocket 断开后靠 `/history` 收口。
5. CLI 与 TUI 同时操作，旧 revision 审批被拒绝。
6. 三个 audition 与一个 final 的成本和质量对照。
7. 十镜队列暂停、恢复、单镜重拍和 build。
8. 磁盘低水位、无效输出和 ComfyUI 离线恢复。
9. 未批准或 schema 无效的 ScriptBundle 不能创建任何 H3 job。

硬件测试产生的 workflow、环境和 profile 指纹写入 benchmark run，不把某次成功口头当成长期能力。

## 23. 实施顺序

### Phase 0：冻结事实（合同与 H3 T2V 事实已完成）

- 冻结 CreativeBrief / ScriptBundle 边界和 `screenwriter` skill 的最小输出
- 导出 H3 API workflow
- 核实节点 binding、输入输出和 T2V capability
- 记录现有手工运行的观察值，但不把它冒充生产 benchmark

### Phase 1：领域与存储（已完成）

- Cargo 工程、schema、ID、hash、原子存储和项目锁
- brief / script bundle / authoring receipt / bible index / shot / state / approval / job / attempt / take 类型
- 只读 `sparkstage preflight`、`sparkstage script validate` 和 store / schema 测试；本阶段不暴露会写状态的 `project new`

### Phase 2：最小 worker 与 ComfyUI（控制面、mock 和真实 T2V binding 已完成）

- Unix socket、命令幂等、revision
- 通过 worker 交付 `sparkstage project new` 和 `sparkstage project status --json`
- 全局队列和单 GPU 资源锁
- `script validate`、`script apply`、文案包批准与原子提升
- adapter 的 submit / WebSocket / history / backend failure / output / recovery

### Phase 2.5：生产链 benchmark（控制面和首轮 DGX T2V 样本已完成）

- `sparkstage benchmark h3` 复用 worker、GPU 锁和 camera adapter
- 建立可信 baseline，再验证 audition / final、attention、compile、cache 和量化 profile
- 原始产物写入应用数据目录，仓库只保留小型汇总结论

### Phase 3：出镜闭环（状态、媒体和 build 逻辑完成，真实 T2V 单镜已验收）

- audition、select、promote、direct render、retry、approve
- ffprobe 硬检查、take 血缘、首尾帧和报告
- draft cut、trailer 和 final build，以及确定性 SRT/VTT 交付
- 角色/地点参考的不可变导入、替换历史、hash 校验和精确 stale 传播

### Phase 4：Ratatui（已完成）

- TerminalGuard、IPC snapshot 和事件订阅
- Projects、Dashboard、Review、Shots、Takes、Queue、Builds、Storage、History、Diagnostics
- 外部预览、revision conflict 和窄终端退化

### Phase 5：验证（无 GPU 回归已完成，产品与 DGX 验证待执行）

- 《雨夜公寓》三镜试拍与十镜闭环
- 故障注入
- 3 部 / 30 镜产品验证
- 决定是否进入 v1 审片页和自动语义检查

### 23.1 当前实现边界

截至 2026-08-29，本地代码与无 GPU 测试已覆盖项目存储、worker IPC、文案合同批准、队列暂停/恢复/取消、候选与 take 决策、自动 audition 批次、build 执行与恢复、参考素材管理与精确 stale 传播、SRT/VTT 字幕、人工终片 gate、预算估算/审批/磁盘硬线、两阶段 decision 恢复、可恢复清理、七类本机里程碑 hook、Ratatui 10 页控制面、项目 TAR 归档/无覆盖导入和 `0.9 -> 1.0` 迁移备份。两个独立完整 ScriptBundle fixture 与一个拒绝样例通过固定 contract suite 和稳定评测报告；ComfyUI mock 已验证 `/prompt` request identity、`/history` 成功与执行失败、WebSocket 断线后的 history 收口、非法输出路径拒绝和全局 interrupt 协议。DGX Spark 上的生产 worker 还完成了一次真实 H3 T2V smoke take，验证了 prompt、seed、输出前缀、模型指纹、原生音轨、媒体硬检查和完整 job/take 血缘。该单次结果不代表性能基线、其它生成操作或外部模型文案质量已经验证。

仍需在 DGX Spark 完成：分别实测 I2V / FLF2V / R2V，用真实 H3 素材验证完整两镜 FFmpeg build 与联系表产物，并记录 audition/final 的冷启动、稳态耗时、显存和质量基线。除已通过烟测的 T2V 外，相关 capability 必须保持未验证或禁用；在性能样本形成有效基线前，预算 source 保持 `unmeasured_default_v1`。

## 24. 仍需实测的事实

以下不是架构讨论题，必须从当前 DGX Spark 与 workflow 得到答案：

- 当前工作流的 I2V、FLF2V 和 R2V conditioning 是否会真实影响输出
- 参考视频经 `LoadVideo -> GetVideoComponents` 转为 H3 IMAGE 帧流后的质量与成本
- ComfyUI 是由 SparkStage 托管还是作为 attached 服务运行
- audition profile 的分辨率、帧数、步数和三抽成本
- DGX Spark 上 Ratatui / Crossterm 和播放器命令的实际兼容性

这些事实确认后写入 adapter capability report、profile 和 `Cargo.lock`，不回填成模型家族的泛化承诺。
