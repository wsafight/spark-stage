# 运行手册

## 1. 前置条件

- Rust `1.98.0`，仓库已通过 `rust-toolchain.toml` 固定。
- 本机安装 `ffmpeg` 和 `ffprobe`。
- 生产拍摄需要运行中的 ComfyUI；当前 H3 默认 endpoint 是 `http://127.0.0.1:8188`。
- 生产 adapter 必须先通过 `sparkstage preflight`，未烟测操作不能进入普通生成路径。

先执行：

```bash
cargo run -- preflight --adapter-config adapters/minimax-h3-comfy.yaml --json
```

`ready: true` 只表示本机运行时、workflow、节点输入和已记录能力满足投递前条件，不代表未认证操作已经可用。

## 2. 从 brief 到合同

外部 Agent 按 `skills/screenwriter/SKILL.md` 生成 ScriptBundle。SparkStage 不直接调用语言模型，先离线验证：

```bash
cargo run -- script validate path/to/script-bundle.json
```

验证通过后通过 worker 导入并请求审批：

```bash
cargo run -- script apply path/to/script-bundle.json --project PROJECT_ID
cargo run -- script approve --project PROJECT_ID
```

审批前不会创建 H3 job。合同被替换时，旧合同保留在项目目录和决策历史中。

## 3. 参考素材

参考图和地点图必须先导入项目，不建议直接把源码路径写进 shot：

```bash
cargo run -- refs import \
  --project PROJECT_ID \
  --kind character \
  --id CHARACTER_ID \
  --file portrait.png

cargo run -- refs impact \
  --project PROJECT_ID \
  --kind character \
  --id CHARACTER_ID
```

替换已有参考素材前，先检查影响范围；有生产 take 或 build 时必须显式 `--accept-impact`。旧参考不会被覆盖，依赖它的 take/build 会按实际引用精确标记 stale。

## 4. 启动 worker

```bash
cargo run -- worker run \
  --data-dir "$SPARKSTAGE_DATA_DIR" \
  --adapter-config adapters/minimax-h3-comfy.yaml
```

CLI 和 TUI 通过 worker 的 Unix socket 发送命令。常用观察命令：

```bash
cargo run -- project list --data-dir "$SPARKSTAGE_DATA_DIR"
cargo run -- project status --project PROJECT_ID --data-dir "$SPARKSTAGE_DATA_DIR"
cargo run -- queue list --data-dir "$SPARKSTAGE_DATA_DIR"
```

## 5. 普通拍摄与 smoke-test

正式能力使用 `audition` 或 `render`：

```bash
cargo run -- shots audition --project PROJECT_ID --shot S01
cargo run -- shots render --project PROJECT_ID --shot S01
```

首次认证某个尚未验证的 adapter 操作，必须明确使用 smoke-test：

```bash
cargo run -- shots smoke-test \
  --project PROJECT_ID \
  --shot S01 \
  --seed 6049946667774612715 \
  --accept-unverified
```

`--accept-unverified` 只豁免“尚未记录验证”的能力检查，不豁免文件安全、媒体质量、预算、revision 或合同校验。smoke-test 策略会写入 job journal，worker 重启恢复时仍按同一策略执行。

## 6. 审片、重试和晋级

生成通过媒体硬检查后，系统创建候选和 blocking approval。先选择 take，再批准：

```bash
cargo run -- shots select --project PROJECT_ID --shot S01 --take TAKE_ID
cargo run -- shots approve --project PROJECT_ID --shot S01 --take TAKE_ID
```

单镜失败时只重试该镜头：

```bash
cargo run -- shots retry --project PROJECT_ID --shot S01
```

已批准 take 不能被普通 retry 覆盖；需要新的合同、参考素材或明确决策，旧结果仍保留为历史证据。

## 7. Build 与字幕

Build 从已选 / 已批准 take 生成，不直接读取未验证的 staging 文件：

```bash
cargo run -- edit build --project PROJECT_ID --kind draft
cargo run -- edit build --project PROJECT_ID --kind final
```

含对白时，build 目录和 delivery 目录会生成匹配的 `.srt` 与 `.vtt`。final 必须覆盖完整合同；只选部分镜头时只能做 draft：

```bash
cargo run -- edit build --project PROJECT_ID --kind draft --shots S04-S07,S10
```

## 8. 停止、恢复和未知提交

- `project pause` 只阻止该项目新的 GPU job，不中断正在运行的任务。
- 全局 `queue pause` 阻止新的任务进入执行器，同样不隐式终止当前任务。
- worker 重启后会从 job journal 和 queue snapshot 恢复 queued / running 状态。
- 如果提交已经开始但 backend id 没写回，任务进入 `SUBMISSION_UNKNOWN`，先通过 ComfyUI `/history` 和 job journal 对账，再人工决定后续动作。
- 生成失败的 staging 文件默认保留，使用可恢复 storage cleanup 处理，不直接 `rm`。

## 9. 归档与迁移

```bash
cargo run -- project verify --project PROJECT_ID --data-dir "$SPARKSTAGE_DATA_DIR"
cargo run -- project export --project PROJECT_ID --output project.sparkstage.tar
cargo run -- project verify-archive --archive project.sparkstage.tar
cargo run -- project import --archive project.sparkstage.tar --data-dir OTHER_DATA_DIR
cargo run -- project migrate --project PROJECT_ID --data-dir "$SPARKSTAGE_DATA_DIR"
```

导入拒绝 symlink、hash 不匹配、目标已存在或归档位于项目内部。迁移默认 dry-run，`--apply` 写入前创建备份。

## 10. 生产排障顺序

```text
1. preflight --json
2. project status / queue list
3. 读取 jobs/<job-id>.json 的 attempt 状态
4. 检查 ComfyUI /queue 和 /history
5. 检查 raw/<shot>/.staging 中是否有保留输出
6. 运行 project verify，确认 state / contract / media hash
7. 只有确认 backend 终态后才 retry
```

不要通过直接编辑 `state.json`、删除 job journal 或手工调用 ComfyUI 绕过控制面；这样会破坏 revision、预算和血缘，后续恢复无法判断真实状态。
