# SparkStage P0 产品功能开发清单

**日期**：2026-08-26
**范围**：无需 MiniMax H3 实机即可完成的产品控制面
**原则**：项目运行态写操作经过 worker、项目锁、expected revision 和原子落盘；显式离线维护命令只能做校验、归档、无覆盖导入或带备份的 schema 迁移；adapter/benchmark 等机器级产物不伪造运行能力；任何清理均可恢复。

## 1. 实施顺序

| 顺序 | 功能 | 状态 | 验收结果 |
| --- | --- | --- | --- |
| 1 | 项目列表、项目级暂停和恢复 | 已实现 | 多项目可发现；暂停项目不再启动新 GPU job；运行中的 job 不被隐式中断 |
| 2 | 存储状态、清理计划、应用和恢复 | 已实现 | 只处理 stale/rejected 且未批准的派生产物；先计划、后移动到项目回收区；可按 plan 恢复 |
| 3 | 批量审片 | 已实现 | 一次 revision 提交多个 shot/take 选择；全部预检成功后才原子提交；已知 warning 必须显式接受 |
| 4 | 决策历史 | 已实现 | CLI 能按追加顺序倒序读取 decision journal，不从日志推断审批 |
| 5 | H3 benchmark 控制面 | 已实现 | 生成带环境、adapter/workflow/profile 指纹的 prepared run；只追加带 job/evidence 的 DGX 样本，不自动标 verified |
| 6 | Adapter 配置向导 | 已实现 | 从 ComfyUI API workflow 和显式 binding 生成禁用 YAML；不自动猜测或声明未烟测能力 |
| 7 | 项目预算合同 | 已实现 | 显示 unmeasured 来源；排队前检查时间/take/磁盘；超限审批按合同 revision 授权；磁盘硬线不可绕过 |
| 8 | Ratatui P0 控制台 | 已实现 | 10 个页面共用 worker 命令面；项目切换、批量审片、预算、清理与 committed history 均可操作 |
| 9 | 崩溃一致性与审计恢复 | 已实现 | decision 两阶段 prepared/committed；prepared 不对外显示；cleanup apply/restore 可从实际文件位置续跑 |
| 10 | 项目可移植性 | 已实现 | 项目校验、逐文件 SHA-256 TAR、verify-before-import、禁止覆盖、迁移 dry-run 和修改前备份 |
| 11 | 自动化质量门禁 | 已实现 | Linux/macOS CI、标准 FFmpeg 合成媒体、65% 行覆盖率、aarch64 check、audit/deny 和 Agent 合同夹具 |

## 2. 命令面

### 2.1 项目管理

```text
sparkstage project list
sparkstage project pause --project PROJECT_ID
sparkstage project resume --project PROJECT_ID
```

`project pause` 是项目级调度门：阻止该项目新的 pending job 占用 GPU。它不取消当前运行 job，也不等同于全局 `queue pause`。

### 2.2 安全存储

```text
sparkstage storage status --project PROJECT_ID
sparkstage storage plan --project PROJECT_ID
sparkstage storage apply --project PROJECT_ID --plan PLAN_ID
sparkstage storage restore --project PROJECT_ID --plan PLAN_ID
```

计划只包含项目根内的普通文件。`apply` 使用原子 rename 移到 `trash/<plan-id>/files/`，不执行永久删除；`restore` 在原路径被占用时停止，不覆盖新文件。

### 2.3 批量审片

```text
sparkstage shots review --project PROJECT_ID --file review.json [--approve]
```

输入示例：

```json
[
  {"shot_id":"S01","take_id":"TAKE-...","accept_warnings":false},
  {"shot_id":"S02","take_id":"TAKE-...","accept_warnings":true}
]
```

请求先验证全部项目；任一项失败时不改变任何 shot。`--approve` 同时把选择推进为批准，否则只完成选择。

### 2.4 决策历史

```text
sparkstage history decisions --project PROJECT_ID --limit 50
```

机器输出保留 event id、kind、subject id、command id 和时间。默认按新到旧展示。

### 2.5 Benchmark

```text
sparkstage benchmark h3 init --adapter-config PATH [--environment-file environment.json]
sparkstage benchmark h3 record --run RUN_ID --sample sample.json
sparkstage benchmark h3 show --run RUN_ID
```

本地代码负责 run 身份、输入校验、文件布局和不可变指纹。`record` 只导入已有生产 job 的原始观测，并要求 evidence 路径；它不启动或绕过 worker 的 GPU job。没有 DGX 实测与完整复核时 run 始终是 `prepared`，不能写成 `verified`。

### 2.6 Adapter 配置

```text
sparkstage adapter scaffold \
  --workflow workflow-api.json \
  --output adapter.yaml \
  --endpoint http://127.0.0.1:8188 \
  --output-node 120 \
  --model-fingerprint MODEL_HASH \
  --prompt 45.text \
  --seed 78.noise_seed \
  --output-prefix 120.filename_prefix \
  --binding first_frame=90.image
```

向导逐项确认节点与 input 存在，并拒绝重名或重复指向同一 input 的 binding。输出默认不覆盖已有配置，生成的 adapter 保持禁用、`verified_operations` 为空。只有 DGX 上通过 `preflight` 和最小烟测后才能人工启用并记录能力。

### 2.7 预算合同

```text
sparkstage budget status --project PROJECT_ID
sparkstage budget default --output budget.json
sparkstage budget apply --project PROJECT_ID --contract budget.json
```

默认 source 为 `unmeasured_default_v1`：4 小时、每镜最多 3 个 audition / 2 个 final、最低剩余磁盘 5 GiB。时间或 take 超限创建 blocking approval；批准只对当前合同 revision 和具体维度生效。最低磁盘线返回 `DISK_BUDGET_EXCEEDED`，不能通过审批绕过。DGX benchmark 前这些参数只能称为保守估算。

### 2.8 项目校验、归档与迁移

```text
sparkstage project verify --project PROJECT_ID
sparkstage project export --project PROJECT_ID --output project.sparkstage.tar
sparkstage project verify-archive --archive project.sparkstage.tar
sparkstage project import --archive project.sparkstage.tar
sparkstage project migrate --project PROJECT_ID
sparkstage project migrate --project PROJECT_ID --apply
```

归档拒绝 symlink、非普通文件、目标位于项目内部和已存在输出；manifest 固化每个相对路径、字节数和 SHA-256。导入在隐藏暂存目录里先复核 payload、brief、state 和合同 hash，再发布到目标 ID；目标已存在时永不覆盖。迁移默认 dry-run，当前只支持 `0.9 -> 1.0` schema 更新，`--apply` 在写任何项目文件前备份原始 `project.json`、`state.json` 和计划；未知 schema 只报告手工修复，不猜测。

### 2.9 Ratatui 控制台

```text
sparkstage tui [--project PROJECT_ID]
```

页面固定为 Projects、Dashboard、Review、Shots、Takes、Queue、Builds、Storage、History、Diagnostics。项目切换会重建 revision subscription；批量批准 warning 前必须显式接受。History 只显示 committed decision，Storage 的 apply/restore 都要求确认并复用 worker revision。

## 3. DGX 验证边界

以下不属于本地完成声明：MiniMax H3 的真实节点 binding、原生音轨、T2V/I2V/FLF2V/R2V、audition/final 成本比、显存、画质和完整 FFmpeg 交付。它们必须在 DGX Spark 上产生 capability report 与 benchmark run 后才能标为 verified。

## 4. 测试要求

- 每个新命令覆盖成功、revision conflict、非法路径和重复命令。
- 批量操作覆盖全成全败，禁止部分提交。
- 清理覆盖计划篡改、目标冲突、重复 apply 和 restore。
- benchmark 与 adapter 向导不访问 GPU，也不把 mock 结果写成 verified。
- decision prepared/committed、重复 event、原子批量和 cleanup 中断恢复必须覆盖故障注入。
- 项目归档覆盖 payload 篡改、symlink、目标冲突、导入拒绝覆盖和迁移备份。
- 两套独立完整 ScriptBundle 与拒绝样例使用固定 expectation 回归，不在测试里调用 LLM。
- 标准 FFmpeg 环境执行无音轨、静音、黑帧、静帧、时长、FPS、边界帧和两镜 build；能力不足只能在夹具生成前跳过。
- 全量 `cargo test --all-targets`、严格 Clippy、65% 行覆盖率、cargo-audit 和 cargo-deny 必须通过。
