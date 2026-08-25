# SparkStage 产品文档

**版本**：0.9<br>
**日期**：2026-08-26<br>
**状态**：MVP 控制面 P0 已完成，无 GPU 回归持续通过；DGX Spark / H3 实机闭环待验证<br>
**运行基座**：NVIDIA DGX Spark（GB10）<br>
**控制面工具链**：Rust 1.98.0，edition 2024<br>
**当前生成栈**：MiniMax H3，由本机 ComfyUI 调度<br>
**生成后端**：可替换。H3 是当前默认摄影机，不写进产品内核和分镜合同。<br>
**当前编剧方案**：外部大模型 Agent + 版本化编剧 skill；SparkStage 不在 DGX 上常驻文案模型。<br>
**第一垂类验证**：中文对白短剧《雨夜公寓》

**性能与质量优化**：[optimization.md](optimization.md)
**技术设计**：[technical.md](technical.md)

---

## 1. 一句话

SparkStage 是运行在 NVIDIA DGX Spark 上的**智能体制片操作系统**：用户把一句 brief 交给外部大模型 Agent，Agent 按 SparkStage 的编剧 skill 生成可拍合同，MiniMax H3 是当前默认摄影机，最后吐出一条能播的成片。

它不是又一个 ComfyUI 工作流，也不是 Runway / 可灵的网页套壳。它是把「选题 → 剧本 → 分镜 → 出镜 → 审片 → 插片 → 成片」收成一条可暂停、可重跑、可换垂类、可换模型的生产线。

### 1.1 产品承诺

给外部 Agent 一句话和一个成片规格，它按 SparkStage 的 skill 生成项目、人物、场景、故事和分镜文案包；Rust 校验通过并由用户确认后，DGX 才进入 H3 出镜。第一个可看试拍镜头的 10 分钟目标从文案合同已批准、camera runtime 就绪后开始计算；外部 Agent 响应和 H3 冷启动单独显示。用户确认试拍方向、需要的候选选择和动态草稿后，MVP 可以无人值守地生成终片候选，但必须再经过一次人工全片画面确认，才能标为 `playable` / `done`。中途退出、模型重启或单镜失败，都不应丢掉已经过审的结果。

第一版承诺的是**更少操作、过程可控、结果可恢复**，不是一键生成完美电影。MVP 不承诺商业发行级调色、稳定口型或十镜角色完全一致；这些问题必须如实显示，不能用“已完成”掩盖。

---

## 2. 为什么现在做

三件事同时成立，以前不成立：

1. **本机视频生成已有历史可行性观察**。此前 MiniMax H3 曾产出画面和立体声对白，5 秒 / 960x544 / 12 步约 4 分钟一镜；该数字尚未按当前 adapter、workflow、驱动和 benchmark 合同复测，不能作为已验证基线。以后换别的模型，只要还走同一套分镜合同即可。
2. **编程助手已经能当制片人**。OpenMontage 证明了「YAML 管线 + Markdown 技能 + 薄工具层」比再写一个 GUI 剪辑器更适合 Agent。
3. **缺的是操作系统，不是模型**。桌上已经有散落的出镜脚本和短剧前作，但剧本、角色锁、审片标准、失败重拍、配乐字幕、多项目并行，全是散件。再堆脚本会烂。

OpenMontage 的方向对，默认假设不对：它面向云厂商、库存素材、英文口播。我们面向 **本机视频闭环优先**、**中文对白短片**、**模型可插拔**。第一刀切短剧，因为这条线对身份锁、场面连续和审片最苛刻——做通了，广告、口播、MV 是降难度。

---

## 3. 给谁用

### 3.1 第一用户（就是你）

一个人 + 一台 Spark。不想开 DaVinci，不想点 40 个 Comfy 节点，想说：

> 两个角色，雨夜公寓，她进门发现他还在，十镜，先出对峙那几镜。

然后去干别的。回来看成片，或看「S03 脸漂了，要重拍」。

### 3.2 第二用户（同一套系统，换垂类）

- 悬疑 / 情感短剧
- 产品演示、开发者 Talk
- 本地知识口播（库存镜头 + 生成补镜）

同一套制片内核，不同 Pipeline Manifest，不同摄影机适配器。

### 3.3 明确不做的用户

- 要云端协作时间线的团队剪辑师
- 要商业发行级调色、法务、发行的片厂
- 要把真实人物脸或未成年人送进模型的人

---

## 4. 产品原则

1. **Agent 是编剧，合同是边界**。外部大模型读版本化 skill，提交结构化文案包；CLI / worker 只负责校验、排队、存盘、拼片和探针，不托管语言模型。
2. **视频本机优先，模型可插拔**。SparkStage 本身不保存外部 Agent 的云 Key，视频生成默认零云 Key。新摄影机模型以 adapter 挂上，挂不上不能堵主路径。
3. **分镜是原子，成片是投影**。可重跑的单位永远是一镜，不是整部片子。成片随时能从现有分镜重投影。
4. **先锁身份，再锁场面，再锁运镜**。脸漂了加步数没有用；先裁参考图、改参考生视频、写死服装。
5. **能播比完美重要**。5 秒低分辨率先闭环，再加分辨率、步数、参考图。
6. **内容边界写进管线**。MVP 在故事、角色、素材声明和提示词层拒绝真实公众人物脸、未成年人和暴力伤害；模型输出仍可能偏离提示词，因此画面未经人工过目不能成为 `playable`。v1 只有经过验证的视觉检查器才能参与自动内容门。
7. **媒体本地，文案边界透明**。视频、音频、参考图和生成结果默认不离开 DGX Spark；外部 Agent 默认只接收 brief 与文本合同。任何媒体上传或云端 camera adapter 都必须另行列出数据并取得确认。
8. **重大决定只追加，不静默覆盖**。模型、工作流、审批策略或已确认创意发生变化时，保留旧决定并追加新决定，用户能知道什么时候、为什么变了。
9. **界面是观察窗，不是第二套内核**。CLI、审片页和以后可能出现的桌面壳都读取同一批项目文件；任何界面挂掉，生产仍可续跑。

### 4.1 数据边界

- 本机 adapter 不需要云 Key，项目素材与生产记录只写入 `projects/<id>/`。机器级配置、全局队列和 worker 锁写入独立的 SparkStage 应用数据目录，不复制项目媒体。
- 外部 Agent 的认证与会话由 Agent host 自己管理，SparkStage 不接收或保存其密钥。编剧 skill 默认只允许读取 brief、pipeline 规则、文本 bible 与 schema，不读取 raw、参考图、成片或无关项目。
- 云端 adapter 必须在执行前列出服务商、将上传的文件和预计保留范围，确认只对当前项目生效。
- 日志默认不得写入密钥、完整环境变量或与任务无关的本机路径。
- SparkStage 不自动删除 raw、参考图或失败 take；清理必须先给出可回收空间和精确文件范围。
- 审片页默认只监听本机地址；开放到局域网必须显式开启，并要求访问令牌，不能裸绑 `0.0.0.0`。

### 4.2 来源与发布边界

- 每个生成或外部素材都记录来源、模型、adapter、工作流版本和授权说明；纯本机生成也不能只写一个文件名。
- 用户提供的素材标记为 `user_provided`，系统不猜测其授权状态。
- 云端生成、库存素材和音乐必须在审片报告中保留服务商与许可信息。
- 第三方模型、权重、节点、检查器和后期增强工具同样记录代码许可、权重许可和允许用途；许可未知或限制非商用时，不能进入商业 profile。
- SparkStage 可以交付技术上可播放的文件，但不自动声称拥有公开发行、商用或人物肖像授权。
- 导出前若存在授权未知素材，成片状态最多是 `done_with_warnings`。

---

## 5. 用户看到的产品

没有时间线 GUI。MVP 主路径是外部 Agent + 编剧 skill + CLI / 可选 TUI + 项目目录；用户不需要和 SparkStage 内置聊天，因为它没有内置文案模型。TUI 用来控制生产，不冒充视频播放器。

### 5.1 一次完整下单

用户：

> 做一部 50 秒短剧，雨夜公寓，女室内设计师 + 男音乐制作人，十镜，先出中间对峙。

系统回：

```
项目  rain-apartment
管线  short-drama
规格  960x544  24fps  5s/镜
档位  audition → final（具体采样参数由当前 adapter profile 决定）
分镜  S01..S10  已写进 shots.json
策略  先试拍 S04 S05 S06；高风险镜头各抽 3 个低成本小样
审批  文案包待确认；确认前不创建 H3 job
队列  等待「按这个拍」
预计  确认后第一条小样目标 <10 分钟；候选组和晋级时间由本机基线估算
出品  final/rain-apartment.mp4
```

之后用户可以随时说：

- 「只重拍 S06，种子 +1，改成他已经走到门口」
- 「先给 S06 抽三个小样，我选完再拍大版本」
- 「选 S06-T002，用首尾帧参考晋级」
- 「S03 开衫还在身上，提示词补一句」
- 「先别拼成片，我先看 raw」
- 「拼一个只有 S04–S07 的预告」

### 5.2 项目在磁盘上长什么样

```
<project-root>/rain-apartment/
├── PROJECT.md                 # 一句话、类型、禁区
├── bible/
│   ├── index.json              # 角色 / 地点稳定 ID 与来源索引
│   ├── characters/
│   │   ├── zhao.md             # 单角色身份锁
│   │   └── lin.md
│   ├── locations/
│   │   └── apartment-living-room.md
│   └── style.md
├── script/
│   ├── brief.md               # 用户原始需求；后续生成不能覆盖
│   ├── story.md
│   ├── shot-list.md
│   ├── shots.json             # 原子分镜
│   └── authoring.json         # skill / schema / Agent 信息与输入输出 hash
├── refs/
│   ├── characters/            # 过审定妆
│   ├── locations/             # 过审场景板
│   └── shots/                 # 可选的镜头首帧 / 目标尾帧
├── raw/
│   └── S06/
│       ├── S06-T001.mp4
│       └── S06-T001.json      # 本 take 的完整血缘和审片结果
├── jobs/                      # 投递前先落盘的 job journal，供崩溃恢复
├── review/
│   ├── draft-cut.mp4
│   ├── contact-sheet.jpg
│   ├── report.md
│   ├── S06/
│   │   ├── S06-T001-first.jpg
│   │   ├── S06-T001-last.jpg
│   │   └── S06-T001-handoff.jpg
│   └── boundaries/            # 相邻镜头首尾帧对照
├── final/                     # 成片
├── history/                   # 被新版本替代的合同和状态快照
├── state.json                 # 当前阶段、镜头状态和选择 / 审批结果
├── decisions.jsonl            # 追加式决策与人工审批历史
├── events.jsonl               # 机器事件流，供 CLI / 审片页观察
└── logs/
```

项目内容都落在这些文件里，不另搞数据库。Agent 可以提出内容修改，但 `state.json`、`decisions.jsonl`、`events.jsonl` 和 `jobs/` 只能由 `sparkstage` 写入；`state.json` 是当前项目状态的唯一真相，另外两个日志只保留历史，不反向驱动状态。

默认 project root 位于 SparkStage 应用数据目录而不是源码仓库；开发时即使把它改到仓库根，`/projects/` 也必须被 Git 忽略。跨项目的 GPU 排队不能塞进某一个项目的 `state.json`。SparkStage 应用数据目录另存一份机器级 `runtime/queue.json` 和 worker 锁；它们只管理 job 引用和资源所有权，项目合同与审批结果仍留在项目目录。

### 5.3 五个阶段（对用户只暴露这些词）

| 阶段 | 人要做什么 | 机器做什么 |
| --- | --- | --- |
| 立项 | 一句话 + 垂类 + 规格 | 建项目、选管线、保存 brief |
| 编剧 | 在 Agent 中给 brief / 改稿意见，再审一页文案包 | Agent 按 skill 出 bundle；Rust 校验并落成 PROJECT + bible + story + shots |
| 出镜 | 选择小样或让系统代选 | 低成本试镜、候选晋级、调摄影机 adapter，写 raw |
| 审片 | 说「过 / 重拍 / 先这样」 | 抽帧、对照 bible、标失败码 |
| 成片 | 要不要字幕、预告还是正片 | trim + concat + loudnorm + 包装 |

每个阶段可单独重入。出镜中途可以改后面还没拍的提示词，不影响已过审的 raw。

外部大模型可以按 skill 分几步完成大纲、人物 / 地点 bible 和逐镜合同，也可以根据 `script validate` 返回的字段路径修正，但 SparkStage 不管理这些推理过程。Agent 最终只能提交一个 `ScriptBundle`，不能生成或改写 ComfyUI workflow，也不能绕过 schema、对白时长、内容输入和预算检查。文案包批准后才允许排队出镜。

MVP 交付 skill，不维护另一份孤立的“万能 prompt”。`screenwriter` skill 内含入口提示、分步编剧规则、schema 引用、合法 / 非法示例和校验失败后的修正规则；需要粘贴到普通聊天窗口时，可以从同一 skill 导出简化 prompt，但它不是第二份规范。

### 5.4 两种工作模式

| 模式 | 系统怎么跑 | 适合什么时候 |
| --- | --- | --- |
| 快速模式 | 确认试拍方向后按已验证规则筛选候选、有限重拍并成片；置信度不足仍停下问人 | 已经跑通的项目和批量生产 |
| 导演模式 | 在人设、定妆、分镜、候选选择和终片节点等待确认 | 新故事、新角色或新模型 |

新项目默认用导演模式跑到第一批试拍；方向确认后，用户可以说「后面自动跑完」切到快速模式。两种模式使用同一份项目状态，切换不会重建项目或丢镜头。

### 5.5 必须停下来问人的情况

无论哪种模式，遇到以下情况都不能自行越过：

- 内容安全规则触发
- 首次启用会把素材送出本机的云端 adapter
- 预计耗时、磁盘占用或重拍次数超过项目预算
- 自动重拍达到上限，仍然没有达到目标质量
- 候选之间没有明显优胜者，或晋级只能使用比已批准方案更弱的策略
- 修改会推翻已经确认的人设、定妆或已过审镜头

每次停下只给当前场景需要的少量明确动作：候选阶段是 `选择 take`、`继续抽`、`改合同`、`停止`；审片阶段是 `通过`、`重拍`、`接受警告`、`停止`。每个动作都说明会影响哪些镜头、采用什么晋级策略和预计增加多少时间。

### 5.6 一次任务交付什么

成片不是唯一交付物。一次完整任务至少留下：

```
final/<project>.mp4              # 正片
final/<project>-trailer.mp4      # 要求预告时生成
review/draft-cut.mp4             # 由选中小样组成的低规格动态草稿
review/contact-sheet.jpg         # 全片分镜缩略图
review/report.md                 # 候选选择、晋级血缘、质量状态和遗留问题
state.json                       # 可续跑的当前状态
```

项目结果与成片质量等级是两件事。用户看到的项目结果分为：

- `in_progress`：仍在生成、审片或成片
- `done`：目标成片已生成，所有必过项满足要求
- `done_with_warnings`：成片可看，但报告中仍有用户接受的非阻断问题
- `needs_review`：机器无法替用户决定，项目停在明确的审片动作上
- `failed`：发生无法自动恢复的运行错误，已保留可续跑信息
- `cancelled`：用户主动停止，已完成素材仍保留

只要还有 blocking approval，项目结果就必须是 `needs_review`；`done` 和 `done_with_warnings` 都要求阻断审批已清空，区别只在是否保留已由用户接受的非阻断警告。

`draft_cut`、`playable`、`approved` 是成片质量等级，不作为项目运行状态使用。MVP 的确定性硬检查只能把成片候选推进到 `draft_cut + needs_review`；人工完成一次全片画面与主要情节确认后，才允许升为 `playable`。`approved` 仍表示更高的逐项验收，不由这次最低确认自动获得。

### 5.7 第一次使用的黄金路径

1. 预检 DGX Spark、MiniMax H3 / ComfyUI、ffmpeg 和可用磁盘。
2. 用户把一句 brief 交给外部 Agent；Agent 读取编剧 skill 与 schema，生成 `ScriptBundle`。
3. Agent 调用 `script validate`，按结构化错误修正后再 `script apply`；worker 原子生成项目合同并等待一次文案包确认。
4. 用户一次性确认或要求 Agent 重写 PROJECT、人物、场景、故事与十镜分镜；确认前不启动 H3 任务。
5. 文案包通过后，worker 先给最能暴露身份、对白和场景问题的 3 镜各出一个低成本小样。
6. 用户确认角色和风格；高风险镜头再按预算补抽候选，简单镜头不盲目多抽。
7. 用户选择候选；若某镜只有一个通过硬检查的候选，快速模式可以继续，否则进入候选审批。系统先用选中小样拼动态草稿，由人检查故事和节奏。
8. 动态草稿通过后，按可用晋级策略生成 final take，续跑剩余镜头并有限重拍。
9. 系统无人值守生成终片候选、缩略图总览、边界对照和审片报告，停在一次全片视觉确认；通过后交付 `playable`，项目保持可重入。

### 5.8 稳定的用户词汇和命令面

用户只需要理解项目、分镜、take、试拍、过审、重拍和成片。内部节点名、ComfyUI workflow id、采样器实现和 ffmpeg 参数不得泄漏成日常操作词。

| 用户意图 | 稳定命令面 |
| --- | --- |
| 检查这台机器能不能拍 | `sparkstage preflight` |
| 启动 / 查看本机 worker | `sparkstage worker run`、`sparkstage worker status` |
| 打开终端控制台 | `sparkstage tui` |
| 用 brief 建项目 | `sparkstage project new --brief-file brief.md` |
| 校验 / 导入 Agent 文案包 | `sparkstage script validate bundle.json`、`sparkstage script apply bundle.json` |
| 批准文案包 | `sparkstage script approve` |
| 查看项目 | `sparkstage project status` |
| 给指定镜头抽低成本小样 | `sparkstage shots audition S04-S07 --takes 3` |
| 选中候选并晋级 | `sparkstage shots promote S06 --take S06-T002` |
| 直接拍全部或指定镜头 | `sparkstage shots render`、`sparkstage shots render S04-S07` |
| 重拍 / 通过一个 take | `sparkstage shots retry S06`、`sparkstage shots approve S06 --take S06-T002` |
| 看队列 / 暂停 / 恢复 / 取消任务 | `sparkstage queue list --project <id>`、`sparkstage queue pause --project <id>`、`sparkstage queue resume --project <id>`、`sparkstage queue cancel --project <id> --job <job-id>` |
| 处理通用审批 | `sparkstage approval approve --project <id> --approval <approval-id>` |
| 刷新诊断 / 查看日志 | `sparkstage diagnostics retry --project <id> --probe worker`、`sparkstage logs open --project <id>` |
| 拼局部动态草稿、全片正片或预告 | `sparkstage edit build --kind draft --shots S04-S07`、`sparkstage edit build --kind final`、`sparkstage edit trailer` |
| 看占盘 / 安全清理 | `sparkstage storage status`、`sparkstage storage clean --dry-run` |

Agent 可以替用户调用这些命令，但不能绕开命令面直接改运行态文件。

TUI 首屏固定展示项目阶段、GPU 当前任务、队列、预算、待审批镜头和最近失败。它可以选择候选、通过、重拍、暂停和恢复；预览动作调用用户配置的播放器或打开审片地址。终端不支持图片协议时只显示联系表和文件路径，不能因为预览能力缺失阻塞生产。

### 5.9 四层生产漏斗：先抽小样，再花大机时

| 层级 | 做什么 | 目的 |
| --- | --- | --- |
| 项目试拍 | 选 3 个最容易暴露身份、场景和对白问题的镜头，各出一个低成本小样 | 判断整个项目方向值不值得继续 |
| 镜头试镜（抽卡） | 高风险或主镜头各出 2–4 个低规格候选 take；简单插入镜头默认只出 1 个 | 选择表演、构图、运镜和对白节奏 |
| 动态草稿 | 把选中的小样直接拼成低规格全片 | 在大渲染前检查故事、顺序和节奏 |
| 晋级成片 | 选定候选，按 adapter 能力增强或重拍，再组成正式版本 | 把机时只花在已经证明值得的镜头上 |

抽卡数量不是越多越好。默认根据镜头风险分配：角色近景、双人对白和关键动作可以 3 张；环境、手部或过场通常 1 张；任何镜头达到项目 take 上限都必须停下来问人。

“选中小样”只表示创意方向获批，不表示大视频已经获批。晋级策略按保真程度排序：

1. `enhance`：直接对选中小视频做超分、补帧或画质增强，表演最稳定，但不能修复原有动作和脸部错误。
2. `video_reference`：模型支持 R2V 时，把选中小视频作为动作 / 节奏参考重新生成，保真较好但仍会变化。
3. `frame_reference`：抽取选中小视频的稳定首帧 / 尾帧，用 I2V 或 FLF2V 重拍，能锁构图和动作终点，不能锁完整表演。
4. `seed_replay`：同 prompt、seed 提高规格重拍，只能视为同方向新 take，不能承诺得到同一段视频。

系统必须在晋级前告诉用户实际采用哪一种；不支持前一种时可以推荐下一种，但不能静默降级。晋级后的 take 记录 `parent_take_id` 和 `promotion_strategy`，并重新走完整审片。

项目创建时锁定一个主宽高比和交付规格。MVP 不自动同时生成横版、竖版和方版；改变主宽高比会把所有相关镜头标记为待重拍，而不是在成片阶段强裁。

### 5.10 只在里程碑通知

用户离开机器后，系统只在以下节点通知：第一组候选可选、必须人工决定、任务恢复失败、动态草稿完成、正式成片完成。默认通知是终端状态和本机事件；v1 可增加本机桌面通知或用户配置的命令 hook，不默认接外部消息服务。

通知只是提示，不能充当状态。用户错过、重复收到或关闭通知，都不影响 `state.json` 和审批门；单个镜头的每一步进度不发送通知，避免十镜任务刷屏。

### 5.11 项目预算不是只有钱

本机生成没有 API 单价，但仍消耗时间、显存占用和磁盘。每个项目建立预算合同：预计总墙钟、每镜最大 audition take、每镜最大 final take、最低安全磁盘、是否允许云费用，以及超预算后是停下还是交付当前草稿。

用户不需要手填所有数字；`short-drama` 提供保守默认值，立项时用一句话显示。新增候选、提高 final profile 或全量重拍前重新估算，只展示增量成本。快速模式也不能越过硬预算，未用完的预算不构成继续抽卡的理由。

当前默认合同明确标记为 `unmeasured_default_v1`，不是 H3 实测值：总墙钟 4 小时，每镜最多 3 个 audition take、2 个 final take，最低剩余磁盘 5 GiB；audition / final 暂按每视频秒 30 / 120 墙钟秒和 4 / 12 MiB 估算。时间或 take 超限进入 blocking approval，用户批准具体超限维度后才能重试；最低磁盘线是不可绕过的硬停止。`budget status/default/apply` 和 TUI 都显示估算来源，DGX benchmark 完成后再替换参数。

---

## 6. 能力地图

### 6.1 第一刀就要有（MVP）

- 一条 `preflight` 完成本机运行时、模型、ffmpeg 和磁盘检查
- `preflight` 输出每个 adapter 的能力矩阵，不把云端 H3 的能力误认为本机工作流能力
- 项目创建 / 列状态 / 停 / 续
- 版本化 `screenwriter` skill、`ScriptBundle` schema，以及 `script validate / apply / approve` 闭环；不在 DGX 运行文案模型
- 快速模式 / 导演模式，以及必须停下来问人的审批门
- 分镜 JSON 为唯一拍摄合同
- 摄影机 adapter：先接本机 Comfy，接口留成可换
- 单 worker、单 GPU 持久队列、缺镜续跑；托管运行时可以拉起，外部运行时只等待恢复
- 超时任务按原 job id 恢复；提交结果不确定时停止自动重投，避免制造重复任务
- 同一 Rust 可执行文件提供 CLI 和 TUI；TUI 只通过 worker 命令面操作状态
- audition / final 两套 profile，支持小视频抽卡、候选选择和低规格动态草稿
- MVP 至少支持明确标注为“同方向新 take”的 `seed_replay` 晋级；不承诺大小视频完全一致
- 按标称秒数裁切 + 拼接 + 响度
- 只拼子集（例如只要对峙四镜出预告）
- 每个 take 记录输入哈希、adapter、工作流、模型、参数、耗时和种子
- 抽取每个 take 的首帧、尾帧和稳定接力帧，输出相邻镜头边界对照
- 输出缩略图总览和审片报告，区分完成、带警告完成、待审
- 追加式决策历史和结构化事件流
- 基础 stale 规则：某镜合同变化只标记该镜和相关成片，不删除旧 take
- `storage status` 和只预演不删除的安全清理计划
- 脚本 / 提示词内容门：拒绝未成年人、真实公众人物脸和过度暴力；输出画面在 `playable` / `done` 前强制人工过目

### 6.2 紧接着要有（v1）

没有这些，短剧会一直「能出不能认」：

- 角色卡 / 场景板自动出图，过审后改名进 refs
- 参考生视频身份锁，大头裁切；能力不支持时停止并说明，不静默降级纯文生
- 普通镜头可锁首帧；连续动作或匹配剪辑可选锁目标尾帧
- 候选晋级可按能力使用 `enhance`、`video_reference` 或 `frame_reference`，并保留 parent take
- bible、参考素材和成片配方变化沿依赖关系精确传播 stale
- 审片技能：抽帧检查人数、服装、幼态和画面脏字；只有已验证的检查器才参与自动选择
- 边界审片：检查相邻镜头的身份、服装、道具、视线、运动方向和色温
- 重拍策略：改种子 / 改提示词 / 改参考图 / 降分辨率，有上限
- 对白表：每镜零到多句原文，编剧阶段先通过时长预算，后期可烧字幕
- 预告轨和正片轨两套成片配方
- 本机审片页：看缩略图、切换 take、通过、重拍、接受警告；不做时间线
- 本机里程碑通知或用户配置的命令 hook

### 6.3 以后再有（v2）

- 口播 / 纪录片管线（库存检索 + 生成补镜）
- 多模型路由：视频模型出戏，图像模型出定妆
- 配乐库、环境声床、原生音轨和后配乐的混合
- 简单包装：片头标题、淡入淡出、片尾
- 更准确的资源预算：按分辨率 × 秒 × 步数估时和占盘
- 把本机审片页开放到受控局域网，不做云盘
- 输出平台派生版本：在主规格通过后再安全裁切或重构横版 / 竖版
- 多版本、增量或压缩归档与跨多个历史 schema 的迁移；基础校验和 TAR 归档、无覆盖导入、`0.9 -> 1.0` dry-run / 备份迁移已提前交付

### 6.4 永远不是核心

- 像素级时间线编辑器
- 多人实时协作
- 自动发行到平台
- 「一键长电影」

---

## 7. 和别人的关系

| 东西 | 它是什么 | 我们怎么用 |
| --- | --- | --- |
| [OpenMontage 本地参考](../reference/OpenMontage/) | Agent 制片操作系统，云 + 库存优先 | **学架构**：manifest、skill、薄工具、checkpoint、Backlot。不 fork，不形成运行时依赖 |
| ComfyUI | 当前本机推理运行时 | **摄影机驱动之一**。Agent 只打 API，人不进画布 |
| 视频生成模型 | 可替换的摄影机 | adapter 适配。H3 只是现在这台机器上的一个选项 |
| 图像模型 | 本机定妆 | **美术组**。出角色卡和场景板 |
| 《雨夜公寓》 | 第一条短剧项目 | **内核试验田**。成功标准先在这里验收 |
| 《17号储物柜》 | 已有悬疑短剧前作 | 第二条短剧项目的素材，不和内核搅在一起 |
| Runway / 可灵 等 | 云生成 | 可选 adapter，默认关掉 |
| DaVinci / Premiere | 人工精剪 | 成片之后的逃生门，不进主路径 |

一句话：**OpenMontage 是学校，DGX Spark 是片场，MiniMax H3 是当前摄影机，短剧项目是毕业作品，SparkStage 是制片厂。**

### 7.1 从 OpenMontage 学什么，不学什么

**直接吸收的原则：**

- Pipeline Manifest 只声明阶段、工具、验收和审批门，创意判断留在 skill
- adapter 明确声明真实能力、可用状态、成本和限制，选择器只在能力满足时路由
- checkpoint 可恢复，长任务超时后沿原任务 id 继续，不重新付出一次生成成本
- 决策历史追加保存；换模型、运行时或媒介时不能只改最终文件
- 类似 Backlot 的审片页从项目文件派生，是可关闭的观察者，不是状态拥有者

**明确不照搬的部分：**

- 不搬它面向大量云厂商、库存素材和英文口播的默认路径
- 不引入它的 Python tool registry、Remotion / HyperFrames 和多套 composition runtime 作为 MVP 依赖
- 不沿用缺少镜头首尾关系、take 血缘和短剧身份合同的通用 scene schema
- 不把“能加载自定义 ComfyUI workflow”当成“已经支持首帧、尾帧和参考生视频”；必须验证具体节点绑定

`../reference/OpenMontage/` 只用于阅读和对照，已由根 `.gitignore` 排除。SparkStage 的测试和构建不能依赖这个目录存在。

### 7.2 模型职责：四种语言能力不能混用

| 场景 | 当前负责方 | 是否属于 SparkStage 内核 |
| --- | --- | --- |
| 一句 brief 生成项目、bible、故事和 shots | 外部 Agent host，例如 Codex、Claude Code 或 pi，读取 `skills/screenwriter/` | 否；SparkStage 只提供 skill、schema 和校验命令 |
| 可选的多轮改稿 | 同一个外部 Agent；也可一次生成后直接提交 | 否，不要求 SparkStage 内置聊天界面 |
| 成片里角色说出的中文对白 | 当前由 MiniMax H3 原生音频随视频一起生成 | 由 camera adapter 调度，但不另接 TTS |
| 对白是否说对、缺句或不同步 | MVP 做台词时长预算、音轨硬检查和人工确认；v1 以 FunASR 为主、faster-whisper 为辅，并结合 VAD / SyncNet | 是审片能力，不参与生成对白 |

SparkStage 不指定外部 Agent 的具体大模型，只固定 skill 版本和输出合同；Agent 更换后仍需通过同一 schema。H3 工作流里的 text encoder 只是生成模型组件，不承担编剧。Qwen3-VL 负责后续视觉审片，图像模型负责定妆与场景板，Demucs 只在需要时分离音轨；这些都不能被描述成“对白生成模型”。

---

## 8. 产品级技术边界

本章只定义用户可见的技术承诺。进程拓扑、完整状态枚举、IPC、落盘顺序、ComfyUI binding、调度算法和恢复协议统一以 `technical.md` 为准，不在这里复制第二份实现规范。

```
用户 → 外部 Agent + screenwriter skill
                    │ ScriptBundle
                    ▼
             sparkstage 命令面 ← TUI
                    │
                    ▼
             本机 worker ──→ camera adapter / ffmpeg
                    │
                    ▼
           项目状态、take、审片与成片
```

### 8.1 三层，不许搅在一起

1. **技能层**（Markdown / YAML）<br>
   怎么写故事与提示词、怎么审片、什么情况重拍、什么内容拒。外部 Agent 读这个。
2. **控制层**（Rust worker + CLI / TUI）<br>
   一个 `sparkstage` 可执行文件提供稳定命令面、持久队列和审批。命令幂等，默认给人可读输出，传 `--json` 时给 Agent 稳定的结构化结果。
3. **运行时**<br>
   当前生成后端、ffmpeg、磁盘，以及尚未迁移的 Python 工作流。工具之外谁都不许直接碰。

散落的出镜脚本是 adapter 的胚胎，要逐步收进稳定 CLI，不要为了“纯 Rust”重写已经验证可用的生成工作流，也不要再在对话里临时改 JSON。

### 8.2 管线是数据，不是 if-else

```
pipelines/
├── short-drama.yaml          # 第一优先
├── talking-head.yaml         # 以后
└── product-demo.yaml         # 以后
```

每个 yaml 只声明：阶段顺序、默认规格、必填 bible 字段、编剧 skill、审片规则名、成片配方。换垂类 = 加一个 yaml + 一组 skills，不改内核。换外部大模型不改产品配置；换生成模型 = 换 camera adapter，不改 yaml。

### 8.3 状态机

项目阶段、项目结果、镜头阶段、job 状态和 `stale` 分开记录。`needs_review` 只表示当前有一个或多个阻断审批，不会抹掉项目正在出镜还是成片。候选选择、晋级策略、终片 take 审批和全片 `playable` 确认是不同决定，不能用一次“通过”同时覆盖。完整枚举与迁移矩阵只在 `technical.md` 维护。

### 8.4 产品形态：CLI + TUI，后审片页

MVP 交付一个为 DGX Spark `aarch64` 构建的 `sparkstage` 可执行文件，同时提供 CLI、常驻 worker 和可选 TUI；这不等于写成一个 Rust 源文件。所有界面使用同一命令面和同一状态，界面退出不影响生产。

TUI 使用 Ratatui + Crossterm，是队列与审批控制台，不承担准确的视频播放、色彩判断或逐帧比较。MVP 通过外部播放器和生成的联系表完成视觉审片；v1 再由同一个 Rust 核心在 `127.0.0.1` 提供轻量审片页。只有当外部用户确实需要安装、通知和文件选择器时，再用 Tauri 包装这套核心和审片页。任何界面都不能成为模型 adapter 或项目状态的第二套实现。

### 8.5 摄影机能力合同

每个 camera adapter 在排队前报告已经在当前机器、当前 workflow 上验证过的操作、参考能力、晋级方式、限制、运行位置和指纹。云端 H3 的能力不能代替本机 workflow 烟测；请求能力不足时明确失败，不能静默降级。ComfyUI 节点编号和 binding 只存在于 adapter 配置，完整 capability schema 以 `technical.md` 为准。

### 8.6 一台 DGX Spark 的排队规则

- 视频生成、GPU 审片、增强和 benchmark 共用一把机器级 GPU 资源锁，不允许绕过 worker 私自启动竞争任务。
- 同时只跑一个独占 GPU 任务；用户交互任务优先，但已经运行的镜头不被粗暴抢占，等待任务不会永久饥饿。
- 暂停在当前安全边界生效；取消、超时或重启都保留 job 引用和已有产物。
- 后端是否已接收无法确认时停在 `SUBMISSION_UNKNOWN`，绝不盲目重投。
- ETA 来自本机同规格历史，没有足够样本时明确显示为粗估。

### 8.7 决策历史与变更失效

`decisions.jsonl` 是追加式历史。每条记录至少包含时间、类别、对象、旧值、新值、原因、操作者和受影响镜头。模型、workflow、主宽高比、工作模式、质量目标和人工审批都属于必须记录的决定。

合同变化不能靠“看起来差不多”继续用旧产物：

| 变化 | 自动标记为 `stale` | 保留什么 |
| --- | --- | --- |
| 某镜 prompt、首尾帧或参考输入变化 | 该镜现有 take、边界审片、相关成片 | 原 take 和审批历史，不自动删除 |
| 只用新 seed 或 profile 增加一次重拍 | 不自动 stale 旧 take；生成并比较新 take | 旧选择和审批，直到用户改选 |
| 角色、服装、地点或风格 bible 变化 | 所有引用它的镜头；已过审镜头进入人工确认 | 不相关镜头和旧 bible 快照 |
| 原生对白文本变化 | 该镜视频、字幕和成片 | 其它镜头 |
| 镜头顺序、trim、字幕或音乐变化 | 成片配方和 final | 已过审 raw |
| 主宽高比或分辨率变化 | 所有不满足新规格的镜头和 final | 原规格版本 |
| 默认 adapter / workflow 变化 | 不自动否定旧 take；新 take 使用新指纹 | 混用情况写进报告，用户可要求全量重拍 |

`stale` 只表示“不再匹配当前合同”，不是失败，也不是删除。成片默认拒绝混入 stale take，除非用户明确接受并形成一条决策记录。

### 8.8 文件一致性与 schema 迁移

项目文件带 schema 版本并采用可恢复写入。新版本修改旧项目之前先展示迁移计划并保留原文件；未知 schema 或损坏状态只生成修复计划，不猜测人工审批。原子写入、锁和恢复顺序统一由 `technical.md` 规定。

### 8.9 可追溯，不承诺逐像素复现

每个 take 记录 shot 合同哈希、所有参考文件哈希、prompt、seed、adapter 版本、workflow 哈希、模型栈、采样参数、ComfyUI job id 和输出媒体探针。这样可以解释“它是怎么拍出来的”，也可以在运行时仍然存在时重新投递。

生成模型、驱动、算子和硬件状态可能导致同一种子仍有差异，因此产品只承诺：

- `auditable`：输入和环境记录完整，可以追责和比较
- `rerunnable`：所需 workflow、模型和参考素材仍在，可以重新投递
- 不承诺 `bit_exact`：不宣传逐像素复现

审片报告必须显示当前项目是否仍可重跑；模型文件、workflow 或引用素材缺失时，明确列出缺项。

### 8.10 存储生命周期

`storage status` 按不可丢、可重建、可清理三类统计空间。过审参考图、已选 take、final、状态、决策和报告不可丢；代理文件、抽帧缓存和未选失败 take 可以进入清理候选。

`storage clean` 默认只输出计划。用户批准后先移动到项目内 `.trash/<timestamp>/`，不立即永久删除；只有显式 `storage purge` 才清空回收区。开始新镜头前若剩余空间低于“预计输出 + 安全余量”，任务保持排队并报告需要释放的容量。

---

## 9. 数据合同（先冻这一小段）

### 9.1 一镜

```json
{
  "schema_version": "1.0",
  "id": "S06",
  "title": "他走到门口",
  "duration": 5,
  "width": 960,
  "height": 544,
  "fps": 24,
  "operation": "t2v",
  "characters": ["zhao", "lin"],
  "location": "apartment-living-room",
  "camera": {
    "shot_size": "medium",
    "movement": "static",
    "screen_direction": {
      "zhao": "right",
      "lin": "left"
    }
  },
  "conditioning": null,
  "continuity": {
    "from": "S05",
    "relation": "continuous",
    "handoff": "none",
    "must_match": ["wardrobe", "location", "prop_state"],
    "state_in": {
      "zhao.position": "sofa",
      "lin.position": "doorway",
      "door": "closed"
    },
    "state_out": {
      "zhao.position": "by-door",
      "lin.position": "doorway",
      "door": "closed"
    }
  },
  "generation_plan": {
    "risk": "high",
    "audition_takes": 3,
    "audition_profile": "audition",
    "final_profile": "final",
    "promotion": "auto"
  },
  "dialogue": [
    {"who": "zhao", "text": "我下楼。"},
    {"who": "lin", "text": "雨还没停。"}
  ],
  "prompt": "..."
}
```

`shots.json` 只描述要拍什么，不记录任务是否完成；镜头阶段、`job_id`、候选选择、最终批准和失败码统一写进 `state.json`。seed 属于一次实际生成，不属于 shot 合同；`steps`、`scheduler`、ComfyUI node id 等模型专有字段属于 pipeline profile 或 adapter，不能污染分镜合同。字段增减只能改 schema，不许脚本各写各的。

`characters` 必须列出本镜所有出镜角色，包括没有台词或走位记录的人；`location` 必须引用本镜唯一主地点。两者都引用 `bible/index.json` 的稳定 ID。`dialogue[].who`、`camera.screen_direction` 和 continuity 中的角色键必须是 `characters` 的子集；找不到 bible ID、遗漏出镜角色或引用不存在的地点时，分镜在排队前失败。这样角色 / 地点 bible 或对应参考图变化时，系统才能精确找到受影响镜头。

`operation` 是产品级生成意图：`t2v`、`i2v`、`flf2v`、`r2v`，避免和快速 / 导演工作模式混名。adapter 负责映射到具体后端操作；后端不支持时在排队前失败，不能擅自忽略 `conditioning`。

`generation_plan` 只表达风险、候选数量和 profile 名；`width`、`height`、`fps` 是 final 交付目标，audition 可以生成更低规格的代理 take。具体分辨率、步数、量化或采样器由 `short-drama.yaml` 和当前 adapter 解释。`audition` 必须显著少用机时，否则抽 3 个小样可能比直接拍一个成片更贵；是否划算以 DGX Spark 实测基线决定。

不同 `operation` 的必填控制不能混用：`i2v` 要求 `first_frame`，`flf2v` 同时要求 `first_frame` 和 `last_frame`，`r2v` 至少要求 `reference_images` 或 `reference_video` 之一，`t2v` 不要求视觉输入。首尾分镜图可以由角色与场景参考生成，但这些上游来源不等于视频 adapter 同时接收了多参考图。

### 9.2 首尾帧不是每镜都锁

首帧、尾帧和身份参考是三种不同控制：首帧锁构图，尾帧锁动作终点，身份参考锁“是谁”。不能用一张图承担全部职责。

| 镜头关系 | 首帧策略 | 尾帧策略 |
| --- | --- | --- |
| 普通对白 / 正反打 | 独立分镜首帧或身份参考，不接上一镜像素 | 生成后抽取，用于审片 |
| 同一动作跨镜延续 | 使用上一镜末段选出的稳定接力帧 | 为下一镜继续提取接力帧 |
| 明确走位终点 / 匹配剪辑 | 使用过审起始图 | 使用过审目标图，调用 `flf2v` |
| 新场景 / 时间跳转 | 独立生成 | 不要求和下一镜相似 |
| 产品循环动画 | 可把同一张图用作首尾 | 只在明确需要闭环时使用 |

需要明确起止状态且 adapter 已验证支持时，同一份 shot schema 才切换为：

```json
{
  "operation": "flf2v",
  "conditioning": {
    "first_frame": "refs/shots/S06-start.png",
    "last_frame": "refs/shots/S06-end.png",
    "reference_images": [],
    "reference_video": null
  }
}
```

对白短剧默认不把同一张图片同时钉在首尾，否则人物容易在结尾退回起始姿态，表演像循环动画。也不默认把上一镜最后一帧喂给下一镜：正反打本来就需要换角度，逐镜像素接力还会放大脸漂、曝光漂和错误姿态。

接力时不盲取视频最后一帧。系统从最后约 `0.3s` 抽取多帧，排除闭眼、熔脸、运动模糊和曝光突变后选择 `handoff_frame`；下一镜生成后丢弃重复或静止的头部帧。MVP 先做提取与边界审片，v1 才在本机 workflow 能力确认后自动使用首帧 / 尾帧条件。

### 9.3 一个 take 的生成血缘不可变

`raw/S06/S06-T002.json` 记录这次生成实际发生了什么：

```json
{
  "schema_version": "1.0",
  "shot_id": "S06",
  "take_id": "S06-T002",
  "shot_contract_hash": "sha256:...",
  "resolved_prompt": "...",
  "profile": "final",
  "profile_hash": "sha256:...",
  "parent_take_id": "S06-T001",
  "promotion_strategy": "frame_reference",
  "generation": {
    "adapter": "minimax-h3-comfy",
    "adapter_version": "0.1.0",
    "workflow_hash": "sha256:...",
    "model_stack": ["minimax-h3", "text-encoder", "video-vae", "audio-module"],
    "seed": 88018807,
    "job_id": "JOB-01J...",
    "request_id": "01J...",
    "backend_job_id": "comfy-prompt-id"
  },
  "input_hashes": {
    "first_frame": "sha256:...",
    "last_frame": "sha256:..."
  },
  "outputs": {
    "video": "raw/S06/S06-T002.mp4",
    "first_frame": "review/S06/S06-T002-first.jpg",
    "last_frame": "review/S06/S06-T002-last.jpg",
    "handoff_frame": "review/S06/S06-T002-handoff.jpg"
  },
  "review_runs": []
}
```

take 的生成字段不覆盖旧内容；重新审片只追加一条 review run。当前选择了哪个候选、最终批准了哪个 take、是否 stale 仍只由 `state.json` 表示，人工批准同时追加到 `decisions.jsonl`。`job_id` 标识逻辑生成任务，`request_id` 标识其中成功的 submission attempt，`backend_job_id` 在 ComfyUI 确认接收后绑定；三者不能混为一个字段。

### 9.4 失败码（审片与运行）

| 码 | 含义 | 默认动作 |
| --- | --- | --- |
| `FACE_DRIFT` | 和定妆不是同一个人 | 切参考生视频 + 大头参考 |
| `EXTRA_PERSON` | 多出人 | 重拍，提示词写死人数 |
| `WARDROBE` | 衣服换了 | 重拍，锁服装句提前 |
| `MINOR_LOOK` | 幼态 / 校服；MVP 由人工标记，v1 可由已验证检查器提出 | **丢弃，不准当过审** |
| `TEXT_BURN` | 画面里长出字幕 | 重拍，禁字幕句加强 |
| `ANATOMY` | 多余肢体 / 熔脸 | 降复杂度或换种子 |
| `AUDIO_MISS` | 没对白或口型崩 | 对白缩短，重拍 |
| `START_FRAME_MISS` | 开场构图没有遵守首帧 | 检查 I2V 绑定或换参考图 |
| `END_FRAME_MISS` | 没有到达目标尾帧 | 降低动作复杂度或延长镜头 |
| `CONTINUITY_BREAK` | 相邻镜头人物 / 道具 / 场景状态断裂 | 重拍受影响镜头或改用插入镜头 |
| `MOTION_REVERSAL` | 运动方向或视线突然反转 | 修正 screen direction 后重拍 |
| `CAPABILITY_MISS` | 当前 adapter 不支持分镜要求 | 停止排队，选择有效 workflow 或改合同 |
| `SCRIPT_BUNDLE_INVALID` | Agent 文案包不符合 schema 或跨字段规则 | 返回 JSON Pointer 和稳定错误码；修正后重新校验，不创建 H3 job |
| `WORKFLOW_INVALID` | workflow 节点 / 绑定 / 模型不匹配 | 修复 adapter，不重投任务 |
| `SUBMISSION_UNKNOWN` | 后端可能已接收，但 job id 尚未安全落盘 | 用 request 标记对账；无法确认时停下，不自动重投 |
| `BACKEND_FAILED` | `/history` 明确记录节点执行失败 | 保存 attempt 错误；预算允许时创建新 attempt，否则停止 |
| `OUTPUT_INVALID` | 后端完成但输出缺失、越界或媒体探针失败 | 保留 job 记录，拒绝进入审片 |
| `DISK_LOW` | 空间不足以安全完成镜头 | 保持排队，先执行清理计划 |
| `TIMEOUT` | 运行时挂 / 超时 | 有 job id 就恢复查询；确认原任务失败后才新建 take |

`TIMEOUT` 若已经取得后端 job id，默认动作是恢复查询，不是同种子再投；只有确认原任务不存在或失败后才能新建 take。

### 9.5 候选怎么选

MVP 的机器硬门只包含当前能确定性验证的文件、媒体规格、时长、音轨、预算和已声明输入规则。人物数量、幼态、人体错误、画面文字、关键对白是否说对和主要情节是否成立属于人工画面 / 听觉确认；v1 只有在相应 checker 通过标注集验证后，才能自动提出这些失败码。任何来源确认的阻断失败码都让 take 退出排名。

`short-drama` 的 v1 目标是按 100 分给剩余候选排序：

| 维度 | 权重 | 看什么 |
| --- | ---: | --- |
| 身份与年龄感 | 30 | 是否像过审角色、是否明确为成年人 |
| 表演与提示遵从 | 25 | 动作、情绪、台词和镜头意图是否成立 |
| 构图与连续性 | 20 | 景别、视线、screen direction、服装和道具状态 |
| 画面技术质量 | 15 | 熔脸、肢体、抖动、脏字、曝光和压缩问题 |
| 原生音频 | 10 | 对白完整、可听、没有爆音和明显错位 |

MVP 只执行已经验证的媒体硬检查和确定性规则；多个候选都通过而语义检查器尚未验证时，必须进入 `needs_review`，不能伪造精确分数。v1 在各维度检查器通过标注集校准后，快速模式才按上表评分；第一名无阻断问题且领先第二名至少 8 分时可以自动选中，分差不足仍进入 `needs_review`。导演模式始终展示候选，不用分数替用户做创意决定。用户可以选择低分 take，但系统要保留原因和已知警告。

候选之间若画面和动作高度重复，不算有效抽卡。系统在预算内换 seed 补足差异；达到 take 上限仍缺乏多样性就停止，不用更多同质结果制造“有很多选择”的假象。

### 9.6 三档质量，不用一个「完成」糊过去

| 等级 | 用户拿到什么 | 必须满足 |
| --- | --- | --- |
| 草稿 `draft_cut` | 用于快速判断故事和节奏的初拼 | 目标镜头都有可解码素材，允许身份、口型和画面瑕疵 |
| 可看 `playable` | 可以从头看到尾的第一版 | 机器硬检查通过，并由人工确认没有画面安全阻断、音频可听且主要情节能理解 |
| 过审 `approved` | 满足当前项目明确要求的版本 | 人物、服装、场景、对白和技术检查通过，或例外已由用户逐项接受 |

MVP 默认交付路径是无人值守生成 `draft_cut` 候选，再用一次全片人工确认达到 `playable`；v1 才把连续镜头身份一致纳入 `approved`。质量等级属于成片配方，不能把低标准结果静默标成高标准。

### 9.7 失败怎么对用户说

失败消息必须同时包含：停在哪一镜、发生了什么、机器已经做过什么、保留了什么，以及用户现在能选什么。不能只打印堆栈、任务 id 或失败码。

```
S06 已自动重拍 2 次，仍有 FACE_DRIFT。
最好版本已保留为 S06-T002，没有影响其它已过审镜头。
可选：更换参考图后重拍 / 接受当前版本 / 停在这里。
预计再次重拍：当前未校准粗估约 4 分钟，以本机历史样本来源为准。
```

运行时离线时先尝试恢复一次；磁盘不足时不得启动新镜头；用户取消时保留已完成素材并把项目落到可续跑状态。任何部分失败都不能删除已过审的 take。

---

## 10. 第一垂类：中文对白短剧

用来把内核逼到能用，不是产品的全部。

**故事原型**：两个虚构成年人，一个封闭空间，一条未说完的关系，10 镜 × 5 秒。样片是《雨夜公寓》。

**硬约束**（写进 `short-drama` 技能）：

- 角色年龄在设定里明确为成年人，画面不得幼态
- 禁止真实公众人物脸
- 禁止未成年人、校服、童装
- 暴力只允许叙事必要的压迫感，不写伤害过程
- 默认虚构，不承诺可公开发行
- 每镜允许零到多句对白；编剧阶段按标点、字数、profile 语速和镜头头尾呼吸估算对白时长，超出镜头预算时必须缩短台词、拆镜或延长镜头，不能直接送去生成

### 10.1 连续性先记状态，再比较像素

短剧连续性不等于“相邻两帧越像越好”。每镜必须说明从什么状态开始、以什么状态结束，至少覆盖人物位置、服装、手持物、门窗状态、时间和主要光线。下一镜的 `state_in` 必须能接上上一镜的 `state_out`；正反打可以改变构图，但不能让杯子、外套或人物位置凭空变化。

同一场对话先建立 180 度轴线和人物屏幕方向。切到反打时保持视线关系，只有明确越轴镜头才能改变左右关系。边界审片同时看：

- 身份和年龄感是否延续
- 服装、头发、道具和环境状态是否延续
- 人物视线与运动方向是否合理
- 色温、曝光和雨夜环境是否突然改变
- 上一镜动作是否在下一镜得到完成或有意中断

### 10.2 连续性失败的救片顺序

1. 只重拍断裂的一镜，先修状态描述、参考图或 screen direction。
2. 连续动作确实需要时，使用稳定接力帧重拍下一镜。
3. 主镜头难以连接时，插入手部、门把、雨窗或人物反应等叙事有效的 cutaway。
4. 只有时间跳转或情绪段落变化才使用淡黑；普通对白不拿 crossfade 掩盖换脸和穿帮。
5. 两轮仍失败则停在 `needs_review`，让用户选择接受警告、换镜头设计或删镜。

原生对白音轨默认保留。每句对白应在镜头头尾留出短暂呼吸，不把字卡死在剪辑点；编剧校验使用可配置的中文语速和停顿估算值，实际阈值由 H3 样本校准。拼接时先做微淡入淡出和响度统一，只有音质允许时才做 J-cut / L-cut。字幕直接来自 `dialogue` 合同，转写只用于核对模型是否说对，不反过来改原台词。

**验收《雨夜公寓》**：

- 十镜都能独立播
- 至少对峙四镜能看懂是同一对人和同一间房
- 自动拼出 50 秒终片候选，并在一次全片人工确认前保持 `needs_review`
- 重拍 S06 不会把别的镜冲掉
- 全程不必打开生成器画布
- 换一个声明支持同等镜头能力的视频 adapter 后，同一套 `shots.json` 仍能投；能力不足则在排队前明确阻止

角色卡、故事和分镜计划放在配置的 project root 下的 `rain-apartment/`；源码仓库只保留不含大媒体的示例合同或测试 fixture。

---

## 11. 版本切分

### MVP（先闭环，不写新 GUI）

1. Rust 工程骨架和带版本的 script bundle / shot / state / take schema
2. `screenwriter` skill + `script validate / apply / approve`，让外部 Agent 生成文案包但不能写运行态
3. `sparkstage preflight` 如实报告本机 H3 workflow 的能力和缺项
4. ComfyUI / MiniMax H3 adapter 跑通 prompt、seed、输出和 job 恢复；先不假装支持未验证的 I2V / FLF2V
5. 常驻 worker 和机器级单 GPU 持久队列：`sparkstage project status`、`sparkstage shots render`、`sparkstage shots retry`、停、续
6. audition / final profile 实测拉开机时；支持小视频抽卡、候选选择、动态草稿和带警告的 `seed_replay` 晋级
7. 每个 take 写完整血缘，自动抽首帧、尾帧和边界对照
8. `sparkstage edit build --shots S04-S07` 和全片 build，输出响度统一、缩略图和报告
9. 基础 stale 检查、追加式审批历史、磁盘预检和安全失败
10. Ratatui 控制台复用同一 worker 命令面，完成队列、候选、审批、失败和预算操作
11. 《雨夜公寓》作为第一个 project 挂上，先三镜试拍再完整十镜

截至 2026-08-26，以上控制面、合同、worker/IPC、持久队列、take/build 状态、媒体检查、预算、Ratatui、清理恢复、决策历史和 mock ComfyUI 错误路径已有无 GPU 实现与测试；基础项目归档/导入和 schema 迁移也已提前完成。第 3、4、6、7、8、11 项中依赖真实 H3 workflow、音轨、画质、耗时和 DGX 环境的部分仍未验收，不能因 mock 或合成媒体测试通过而视为完成。

**MVP 完成定义**：外部 Agent 用 `screenwriter` skill 从一句 brief 生成通过校验且经批准的文案包，再用低成本候选拼出《雨夜公寓》动态草稿；选中候选后晋级并无人值守生成预告和终片候选。此时项目保持 `needs_review`，必须经过一次全片人工画面确认才能成为 `playable` / `done`。磁盘上出现文案来源记录、两个视频候选、一张缩略图总览、一组边界对照和一份可追溯审片报告。中途重启一次仍能续跑，CLI 和 TUI 看到同一状态，过程不用手改代码、workflow JSON 或状态文件。

### v1

bible + 定妆 + 本机能力验证后的 I2V / FLF2V + 身份参考 + 自动边界审片 + 精确 stale 传播 + 对白字幕 + 本机审片页 + 安全存储清理。

完成定义：同一对角色拍两部短片，脸还认得出来；改变一张角色参考图时，只标记真正受影响的镜头，不破坏其它已过审 take。

### v2

第二条管线（口播或产品演示）证明内核真的和垂类解耦；第二个 adapter 证明内核真的和模型解耦。增加平台派生版本、增量/压缩归档和更多历史 schema 的迁移。是否增加 Tauri 桌面壳，由真实用户安装和审片反馈决定，不作为 v2 的预设目标。

---

## 12. 成功指标

北极星指标是：**产出一版 `playable` 成片需要多少次人工干预**。一次用户回复或一次批量提交计一次；同屏批量选择三个镜头仍算一次，纯查看状态不计。候选选择、接受警告和最终全片画面确认都属于干预。MVP 不用“无人值守”偷换“无人审批”：机器可以独立生成候选，但最低一次视觉确认仍计入干预。不看 Star，先看这台机器能不能持续交付：

| 指标 | MVP | v1 |
| --- | --- | --- |
| 已批准文案包到第一镜进队 | < 2 分钟 | < 1 分钟 |
| camera runtime 就绪到第一个可看镜头 | < 10 分钟 | < 8 分钟 |
| 外部 Agent 首次提交即通过 ScriptBundle 校验 | 记录基线 | ≥ 90% |
| 5s / 960x544 / 12 步单镜墙钟 | 先记录基线 | 不差于基线 |
| 3 个 audition 小样总机时 / 1 个 final take | ≤ 1.0×，否则不默认抽 3 个 | ≤ 0.7× |
| 试拍、候选与动态草稿审批清除后无人值守生成十镜终片候选 | 能，结果停在 `needs_review` | 能，且失败镜自动重拍 ≤ 2 次 |
| 终片候选到 `playable` 的最低人工确认 | 1 次全片画面确认 | 记录自动检查后的人工确认次数 |
| 一部十镜初剪的人工干预 | ≤ 5 次 | ≤ 3 次 |
| 第一 take 达到目标质量的比例 | 记录基线 | ≥ 60% |
| 选中小样后第一次晋级通过率 | 记录基线 | ≥ 70% |
| 三张候选中至少两张有明显表演 / 构图差异 | 记录基线 | ≥ 90% 的抽卡镜头 |
| 快速模式自动选卡与人工复核一致率 | 记录基线 | ≥ 80% |
| 重拍一镜对其它镜的破坏 | 零 | 零 |
| 中断恢复丢失已过审素材 | 零 | 零 |
| 超时 / 重启造成重复 GPU 任务 | 零 | 零 |
| CLI / TUI 显示或审批结果不一致 | 零 | 零 |
| stale take 被误拼入成片 | 零 | 零 |
| take 血缘字段完整率 | 100% | 100% |
| ETA 相对实际墙钟误差 | 记录基线 | 中位数 ≤ 25% |
| 打开生成器画布的次数 | 0 | 0 |
| 身份可辨认的连续镜数 | 不考核 | ≥ 6 |
| 同能力 adapter 是否需要改 shots.json | mock adapter 合同测试不改 | 第二个真实 adapter 不改 |

### 12.1 第一轮产品验证

MVP 不是跑通一次《雨夜公寓》就算成功。第一轮必须完成 3 部不同情节、共至少 30 镜的中文对白短剧，并满足：

- 3 部都从同一句话入口建立项目，没有手改代码或工作流 JSON
- 至少 2 部在三镜试拍、所需候选和动态草稿确认后，无人值守产出完整终片候选并停在 `needs_review`；各经过一次全片人工画面确认后达到 `playable`
- 至少 1 部先由选中的 audition 小样组成动态草稿，再晋级成片；报告能看出节省或浪费了多少 final-profile 机时
- 每部除创意确认外的人工救火不超过 5 次
- 每次暂停、重启和单镜重拍都没有破坏其它已过审镜头
- 全程不打开 ComfyUI 画布，不把素材发送到云端
- 每部都留下成片、缩略图总览、审片报告和可续跑状态
- 随机抽取 5 个 take，都能从报告追到 shot 合同、参考图、workflow、模型栈、seed 和 job id

没有通过这轮验证之前，不增加第二垂类，不做桌面安装器，也不对外承诺“一句话成片”。

### 12.2 必须演练的故障场景

正常出片不是可靠性证明。MVP 验收时必须主动制造并通过：

1. Rust 进程在 ComfyUI 任务运行中退出，重启后沿原 job id 恢复，不重复提交。
2. Rust 进程在 ComfyUI 接收任务但 job id 尚未落盘时退出；恢复后能对账则绑定原任务，不能对账则停在 `SUBMISSION_UNKNOWN`，绝不盲目重投。
3. ComfyUI 离线后恢复，队列保留顺序和重拍次数。
4. 磁盘降到安全线以下，新镜头不启动，已有产物不受影响。
5. 修改 S06 prompt，只让 S06 和相关 final stale；其它镜头仍然过审。
6. 修改角色定妆图，准确列出受影响镜头，并等待用户决定是否推翻旧批准。
7. 只改字幕或剪辑顺序，不重拍 raw。
8. 请求 FLF2V 但当前 workflow 不支持时，在排队前报 `CAPABILITY_MISS`，不降级 T2V。
9. `state.json` 人为损坏后只生成修复计划，不猜测人工审批。
10. TUI 关闭 / 崩溃时，CLI 和生产队列继续工作；TUI 重开后与 CLI 状态一致。
11. 清理流程先进入项目回收区，误选内容仍可恢复。
12. 选中 audition take 后，晋级 take 保留 parent 和 strategy；即使画面变化也不能冒充原小样的高分辨率副本。
13. 终片候选通过全部机器硬检查但没有人工全片画面确认时，仍保持 `draft_cut + needs_review`，不能变成 `playable` / `done`。
14. Agent 提交缺角色引用、对白超时或夹带 workflow 字段的 bundle 时，返回 `SCRIPT_BUNDLE_INVALID`，且队列中没有新增 H3 job。

### 12.3 产品化闸门

- **MVP**：只服务当前这台 DGX Spark 和第一用户，目标是稳定闭环。
- **v1 内测**：内部连续通过 3 部短剧后，再邀请 3–5 位 DGX Spark 用户验证安装、模型路径和审片理解成本。
- **对外产品**：只有多数内测用户不需要作者远程介入就能完成第一部片，才决定做 Tauri 桌面包装、开源发行或付费版本。

商业模式现在不定。先证明“外部 Agent 编剧 + 本机视频制片”比直接操作 ComfyUI 明显少干预，再决定怎么交付给别人。

---

## 13. 风险

- **身份不稳**：纯文生十镜会换脸。v1 之前不要对外说「角色一致」。
- **小样晋级不是确定性放大**：同 seed 提高分辨率或步数仍可能换表演。只有 `enhance` 是直接处理原视频，其余策略都必须生成新 take 并重审。
- **抽卡反而浪费机时**：候选过多或 audition profile 不够便宜，会比直接拍 final 更慢。以实测比值控制默认张数，不满足阈值就退回一镜一候选。
- **逐镜尾帧接力放大漂移**：生成结果中的脸、曝光和构图误差会沿链传播。只对连续动作使用稳定接力帧，普通正反打独立生成。
- **单镜时长**：多数视频模型对 5–15 秒更稳。单镜超 15 秒当未定义。
- **内容合规**：MVP 的技能层只能约束输入，不能保证模型输出；没有人工全片画面确认，不允许标 `playable` / `done`。v1 自动检查也必须先用标注集验证。
- **外部 Agent 数据外发**：默认只给 brief、文本合同、skill 和 schema；不得让编剧 skill 自行读取 raw、参考图、成片或其它项目。用户要求看图改稿时必须单独确认上传范围。
- **Agent 输出漂移**：换模型或升级 Agent 可能产生不同字段与镜头质量。skill 和 schema 必须版本化，任何输出先 `script validate`，不能直接开拍。
- **OpenMontage 幻觉**：大量 skill 假设云和英文口播。照抄会把本机主路径做胖。
- **Agent 直接改项目合同或运行态**：已经发生过。内核冻结后，编剧 Agent 只生成临时 ScriptBundle，再经 `script validate / apply` 导入；不能直接改 active contract、`state.json`、队列或 skill 本身。
- **过早绑死模型**：产品文档和分镜合同里不写死厂商名词，adapter 内部再写。
- **adapter 虚报能力**：模型家族支持不等于当前本机 workflow 支持。能力必须通过节点、模型和最小烟测共同确认。
- **为了 Rust 重写一切**：当前生成工作流已经能出片。MVP 只用 Rust 收控制面，Python 通过 adapter 继续工作，避免语言迁移吞掉产品验证时间。
- **文件状态并发损坏**：Agent、监控和审片页同时写 JSON 会破坏唯一真相。所有写操作只经 Rust、项目锁和原子改名。
- **种子带来虚假的可复现感**：工作流、模型或驱动变化后，同 seed 也可能不同。只承诺可追溯和条件满足时可重跑。
- **授权信息丢失**：生成文件能播放不代表可发布。来源未知的素材必须阻止无警告交付。
- **自动化掩盖质量问题**：能生成文件不等于完成。必须按 `draft_cut`、`playable`、`approved` 分级，并保留带警告完成状态。
- **桌面壳提前变成主产品**：窗口、安装器和设置页很容易吃掉闭环时间。审片页通过验证前，不启动完整桌面端。
- **TUI 变成终端剪辑器**：Ratatui 只承载状态、队列和审批；不做终端内视频解码、时间线或多套业务逻辑。

---

## 14. 现在手里有什么

| 资产 | 位置 | 去向 |
| --- | --- | --- |
| 本机 MiniMax H3 视频工作流 | ComfyUI 用户目录 | 导出 API JSON，建立节点绑定和能力烟测后成为第一个 camera adapter |
| 外部大模型 Agent | Codex、Claude Code 或其它 Agent host | 读取 `screenwriter` skill 生成 ScriptBundle；不进入 DGX GPU 队列 |
| 角色卡出图脚本 | 现有图像工作流 | bible 阶段 |
| 《雨夜公寓》人设 / 故事 / 分镜 | 配置的 project root / `rain-apartment/` | 第一项目；源码仓库只放轻量示例合同 |
| 出镜 / 拼接 / 值守散件 | 桌面旧目录 | 收成 `tools/` |
| 《17号储物柜》前作 | `minimax-h3-short-drama` | 第二条短剧项目，不混进内核 |
| OpenMontage 本地副本 | `../reference/OpenMontage/` | 只学习 manifest、skill、checkpoint、adapter contract 和 Backlot；不参与构建 |
| DGX Spark 历史观察值 | 5 秒 / 960x544 / 12 步约 4 分钟，当前未复测 | 只能作为 benchmark 候选对照，不能标 verified |
| H3 优化验证计划 | `optimization.md` | 管理性能基线、单变量实验、自动审片和 profile 准入 |
| SparkStage 技术设计 | `technical.md` | 约束 Rust worker、Ratatui、状态存储、ComfyUI adapter 和故障恢复 |

---

## 15. 下一步（从 DGX 对接开始）

无需 DGX 的控制面基线已经完成：Rust/worker/IPC、Ratatui 10 页控制台、预算和审批、崩溃恢复、可恢复清理、项目归档迁移、CI/覆盖率/依赖审计，以及两套独立 ScriptBundle 回归夹具。接下来按以下顺序只补真实生产证据：

1. 在 DGX Spark 导出当前 MiniMax H3 ComfyUI API workflow，逐项填写 prompt、seed、输出、首帧、尾帧、参考素材和音频 binding；生成的 adapter 先保持 disabled。
2. 执行最小 T2V 烟测并保存 capability report；I2V / FLF2V / R2V 分别独立验证，未通过的能力保持禁用，不做静默降级。
3. 用生产 worker/adapter 跑冷启动和稳态 baseline，再单变量比较 audition/final、attention、compile、cache 和 FP8；用结果替换 `unmeasured_default_v1` 预算参数。
4. 用真实输出执行首尾/接力帧、音轨、黑帧/静帧/静音、两镜 build、联系表和血缘报告验收；标准 FFmpeg 的合成媒体 CI 是前置回归，不替代 H3 素材验收。
5. 让至少两个真实外部 Agent 会话各生成完整十镜 ScriptBundle，记录首次通过率和修复次数；checked-in fixture 只保证合同回归，不冒充模型质量评测。
6. 把《雨夜公寓》挂成第一个真实 project：三镜试拍 → 小样抽卡 → 动态草稿 → 晋级十镜终片候选，完整演练中断恢复、预算超限、单镜重拍和最终人工画面 gate。
7. 完成 3 部 / 30 镜产品验证后再进入 v1 身份锁、自动审片和本机审片页；Tauri 桌面壳继续由真实内测反馈决定。

第 1–6 项完成前，不对外承诺 H3 能力、角色一致性、约 4 分钟基线或“一句话成片”，也不把第二垂类做进内核。

---

## 16. 术语表

| 词 | 在 SparkStage 里的唯一含义 |
| --- | --- |
| 分镜 `shot` | 可独立生成、重拍、审片和替换的最小叙事单元 |
| take | 某份 shot 合同在某个 adapter / workflow / seed 下的一次实际生成 |
| 小样 `audition` | 用低成本 profile 生成、用于选择方向的候选 take |
| 晋级 `promote` | 从选中小样出发，以增强或重新生成方式得到 final-profile 新 take |
| 参考图 `reference` | 锁身份、地点或风格的素材，不一定成为视频第一帧 |
| 首帧 `first_frame` | 明确约束视频开场构图的输入图 |
| 目标尾帧 `last_frame` | 明确约束动作终点的输入图，只在需要时提供 |
| 接力帧 `handoff_frame` | 从已生成镜头尾部选出的稳定帧，用于连续动作的下一镜 |
| stale | 产物仍在，但已不匹配当前合同，默认不能进入新成片 |
| 过审 `approved` | 用户或规则对当前具体 take 的批准，不自动覆盖其晋级版本 |
| 成片配方 | 镜头顺序、选中 take、trim、字幕、音乐、响度和包装的可重建描述 |
