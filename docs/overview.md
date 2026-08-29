# 项目总览

## 1. SparkStage 是什么

SparkStage 是一个本机优先的 AI 短视频制片控制面。它不负责训练模型，也不把 ComfyUI 节点重写成 Rust；它负责把创意、分镜、生成、审片和交付组织成一条可暂停、可重试、可恢复、可审计的生产线。

当前运行基座是 NVIDIA DGX Spark（GB10）和本机 ComfyUI，默认摄影机是 MiniMax H3。摄影机通过 adapter 接入，产品内核不绑定某个模型，因此后续可以增加其它本机或云端后端。

## 2. 它解决什么问题

一次 AI 视频生成很容易成功，连续做完一部可追溯的短片却不容易。SparkStage 主要解决六个工程问题：

1. **创意如何变成可执行合同**：外部 Agent 输出 `ScriptBundle`，包含人物、地点、镜头、运镜、对白、参考素材和生成档位；Rust 校验字段、关系和安全边界。
2. **如何避免状态被不同入口写乱**：CLI、TUI 和未来的审片页面都通过 Unix socket 调用唯一 worker，worker 是项目状态和 GPU 队列的唯一写入者。
3. **如何让一镜失败不拖垮全片**：shot 是最小可重跑单位；每次生成都有 job journal、request id、backend id 和 take lineage。
4. **如何知道输出真的能用**：生成结果落到 staging 后先执行 ffprobe / ffmpeg 检查，再原子移动为 raw take；失败文件保留用于诊断。
5. **如何保持人物和镜头连续**：参考图不可变保存并带 hash；替换参考图前显示影响范围，只让真实依赖它的 take/build 失效。
6. **如何解释一次交付从哪里来**：合同、prompt、seed、adapter、workflow、model、参考素材、质量检查和 build recipe 全部进入血缘。

## 3. 一次生产流程

```text
brief
  ↓
外部 Agent + screenwriter skill
  ↓
ScriptBundle validate
  ↓
用户批准合同
  ↓
创建 shot / job journal
  ↓
worker 获取 GPU 独占队列
  ↓
adapter 准备并提交 ComfyUI workflow
  ↓
WebSocket 进度 + /history 终态对账
  ↓
下载 staging → 媒体硬检查 → 抽取边界帧
  ↓
raw take + 审批 / 重拍 / 选片
  ↓
BuildRecipe → ffmpeg 拼片 → 字幕 / 联系表 / 报告
  ↓
人工全片确认 → playable / done
```

合同审批以前不会创建 H3 job。生成任务提交前先写入 job journal；如果 worker 在“已提交但尚未记录 backend id”时崩溃，任务进入 `SUBMISSION_UNKNOWN`，系统阻止自动重复投递，等待人工对账。

## 4. 当前能力边界

| 能力 | 当前状态 | 说明 |
| --- | --- | --- |
| H3 T2V | 已验证 | DGX Spark 真实 20 步 smoke，960x544、24fps、124 帧，音视频硬检查通过 |
| H3 R2V | 未认证 | 参考图动态绑定和生成成功，但样本静帧占比 37.1%，被 audition 门禁拒绝 |
| H3 I2V | 未认证 | 当前正式 workflow 没有 ImageToVideo 分支 |
| H3 FLF2V | 未认证 | 当前正式 workflow 没有首尾帧分支 |
| 12 步 audition | 未准入 | 三 seed 平均约快 41%，但 seed=42 存在明显冻结 |
| 多镜完整终片 | 待验证 | build 控制面已实现，仍需要真实两镜及完整人工审片证据 |

“已实现”表示代码路径和离线测试存在；“已验证”必须有当前机器、当前 workflow、当前模型和当前质量证据。

## 5. 适用和不适用

适合一个人使用一台 Spark 制作中文对白短剧、产品演示、知识口播或本地素材补镜。它不承诺商业发行级调色、稳定口型、十镜角色完全一致，也不替用户判断内容是否具有公开发行或肖像授权。

项目媒体默认留在本机。外部 Agent 默认只接收 brief 和文本合同；任何云端 adapter、媒体上传或第三方模型都必须单独声明数据范围和许可。
