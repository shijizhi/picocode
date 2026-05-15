# picocode TODO / 技术债

这个文档用于记录阶段推进中刻意留下的技术债、简化实现和后续补齐项。

原则：

- 凡是当前阶段为了收敛范围而没有做完整的实现，都必须记录到这里。
- 每条 TODO 必须说明影响范围、补齐时机和验收标准。
- TODO 不是“忘记做”，而是“明确知道先不做，并且知道什么时候补”。

## TODO-001: 完整 `.gitignore` 语义支持

状态：partial

来源阶段：v4.1 Read-only Tool Runtime

当前实现：

- [workspace.rs](/Users/jizhishi/mycode/ai/picocode/src/workspace.rs) 中的 `IgnoreRules` 是轻量实现。
- 当前支持精确匹配、目录规则和 `*.ext` 后缀规则。
- 默认忽略 `.git/`、`target/`、`.picocode/`。

遗留问题：

- 尚未完整支持 gitignore 语义。
- 暂不支持复杂 glob、`**`、否定规则 `!`、路径层级细粒度匹配等。
- 后续如果用于大项目搜索或上下文索引，可能出现文件过滤与 Git 实际行为不一致。

影响范围：

- `ls`
- `read`
- `find`
- `grep`
- 后续 workspace 文件索引

补齐时机：

- v5 引入 `find / grep` 前后。
- 或者当 workspace 文件发现能力需要与 Git 行为严格一致时。

建议方案：

- 引入成熟 crate，例如 `ignore`。
- 统一 workspace 遍历、搜索、读取前检查的 ignore 逻辑。
- 保留默认安全忽略项，例如 `.git/`、`.picocode/`。

验收标准：

- 能正确处理普通文件规则。
- 能正确处理目录规则。
- 能正确处理 `*.ext`、`**` 等 glob。
- 能正确处理否定规则 `!`。
- `ls / find / grep` 使用同一套 ignore 逻辑。
- 补充覆盖上述规则的单元测试。

## TODO-005: `find` 匹配能力升级为更接近代码导航的文件发现

状态：partial

来源阶段：v5.1 Find Tool

当前实现：

- [workspace.rs](/Users/jizhishi/mycode/ai/picocode/src/workspace.rs) 中 `Workspace::find` 使用 workspace-relative path 的大小写不敏感子串匹配。
- 返回文件和目录，按路径稳定排序。
- 支持 `path` 限定搜索根、`limit` 截断、基础 ignore 过滤。

遗留问题：

- 暂不支持 glob / regex / fuzzy matching。
- 暂不做相关性排序，结果只按路径排序。
- 暂不区分“只查文件 / 只查目录 / 文件类型过滤”。
- 暂不返回文件大小、最近修改时间等辅助信息。

影响范围：

- `find`
- 后续 `grep`
- 后续 Search Agent
- 后续修改任务的候选文件定位效率

补齐时机：

- v5.2 `grep` 完成后，开始做更完整 Search Agent 前。
- 或者当模型经常因为候选文件太多而需要更强过滤时。

建议方案：

- 为 `find` 增加 `kind=file|dir|all`。
- 支持 `glob` 或 `pattern_mode=substring|glob|regex`。
- 引入简单 ranking：文件名命中优先于路径命中，浅层路径优先，源代码文件优先。
- 与后续 `grep` 共享搜索配置和截断策略。

验收标准：

- 能按文件名、路径、glob 找到稳定候选集。
- 大项目下不会一次返回过多低价值路径。
- Agent 修改代码时能先用 `find` 缩小候选，再用 `read/grep` 精读。

## TODO-006: `grep` 升级为更完整的代码搜索工具

状态：open

来源阶段：v5.2 Grep Tool

当前实现：

- [workspace.rs](/Users/jizhishi/mycode/ai/picocode/src/workspace.rs) 中 `Workspace::grep` 支持 literal 文本搜索。
- 可从文件或目录开始，目录会递归搜索。
- 返回 `path:line:content`，支持 `limit` 截断和 `ignore_case`。
- 跳过 ignored 路径、二进制文件和非 UTF-8 文件。

遗留问题：

- 暂不支持 regex。
- 暂不支持 glob 文件过滤，例如只搜索 `*.rs`。
- 暂不支持 before/after context 行。
- 暂不返回 skipped 文件统计。
- 暂不做非 UTF-8 文本的编码探测和转码；ASCII 文件不会受影响，因为 ASCII 是 UTF-8 子集。
- 当前仍使用轻量 ignore 规则，尚未接入完整 `.gitignore` 语义。

影响范围：

- `grep`
- 后续 Search Agent
- 后续编辑前定位
- 后续上下文压缩和检索排序

补齐时机：

- v5 搜索工具稳定后。
- 或者进入“根据用户需求自动定位修改点”的阶段前。

建议方案：

- 增加 `glob` 参数，限制搜索文件集合。
- 增加 `regex=false|true` 或独立 `pattern_mode`。
- 增加 `context` 参数，返回命中行附近少量上下文。
- 输出 skipped count，让 Agent 知道是否因为二进制/非 UTF-8/ignore 跳过了内容。
- 如果要支持老项目，增加编码探测和安全转码，例如 GBK / Shift-JIS / Latin-1。
- 与 `find` 共用遍历、过滤和排序策略。

验收标准：

- 能在大项目中稳定、快速地定位文本命中。
- 返回结果足够小，适合直接进入 LLM context。
- Agent 能完成 `find -> grep -> read` 的渐进式定位流程。

## TODO-007: 编辑能力从预览过渡到真正应用

状态：partial

来源阶段：v8 Edit Preview

当前实现：

- [workspace.rs](/Users/jizhishi/mycode/ai/picocode/src/workspace.rs) 已能生成 `propose_edit` diff 预览。
- [workspace.rs](/Users/jizhishi/mycode/ai/picocode/src/workspace.rs) 已接入 `apply_edit`，会记录可逆 checkpoint 后再写入。
- [workspace.rs](/Users/jizhishi/mycode/ai/picocode/src/workspace.rs) 已接入 `apply_edits`，支持多文件 batch edit 与冲突检测。
- [tool.rs](/Users/jizhishi/mycode/ai/picocode/src/tool.rs) 已接入 `propose_edit`、`propose_edit_batch`、`apply_patch`、`apply_patch_batch` 与 `rewind_edit` 工具。
- 批量编辑会先做预览，再基于 `base_hash` / `expected_hash` 做冲突检测；一旦发现冲突，会在任何部分写入外泄前整体中止。
- 预览阶段只读，`apply_patch` / `apply_patch_batch` 才会修改 workspace 文件，`rewind_edit` 可回到最近 checkpoint。

遗留问题：

- 还没有更细的冲突解释、交互确认和多文件 diff 面板。
- 还没有把 checkpoint 链和更完整的 rewind / resume UI 串起来。
- 还没有 diff 面板上的交互确认。
- 还没有把批量编辑的冲突原因做成更细的用户可读摘要。

影响范围：

- `propose_edit`
- 后续 `apply_patch`
- `rewind_edit`
- edit-engine
- session 中的 diff 持久化

补齐时机：

- v9 Edit Engine。

建议方案：

- 先用 deterministic preview 确认修改范围。
- 再引入真正的 patch apply。已开始。
- 为每次编辑保留锚点校验与 checkpoint，回退优先依赖 session 中的 edit 链。
- 中断后允许 resume 或对最近一次编辑 rewind，然后继续补全。

验收标准：

- `propose_edit` 只负责预览，不写文件。
- `apply_patch` 负责真正写入并返回结果。
- 失败时能够准确指出找不到目标块或发生冲突。

## TODO-002: Session JSONL 解析升级为正式 JSON parser

状态：done

来源阶段：v4.2 ToolCall / ToolResult 事件

当前实现：

- [session.rs](/Users/jizhishi/mycode/ai/picocode/src/session.rs) 已使用 `serde_json` 生成和解析 JSONL。
- `session_meta`、消息事件、`tool_call`、`tool_result` 都有 round-trip 测试。
- OpenAI-compatible response 中的 `tool_calls` 也已改为 `serde_json` 解析。

遗留问题：

- 事件结构还没有直接 derive serde，当前仍是手动映射到 `serde_json::Value`。
- 未来事件类型增多时，可以进一步引入专门的序列化 DTO。

影响范围：

- session 保存
- session 恢复
- `--replay`
- 后续工具事件
- 后续 diff / command / file edit 事件

补齐时机：

- v4.3 Agent Loop 最小工具调用前后。
- 或者在新增更复杂事件类型，例如 diff、command output、tool schema 之前。

建议方案：

- 引入 `serde_json`。
- 为 `SessionLine / SessionItem / Event / EventMsg` 增加正式序列化结构。
- 保持 Codex-style JSONL 外层格式：`timestamp + type + payload`。
- 保留向后兼容读取当前 v1 session 格式的策略，或者明确提升 format_version。

验收标准：

- 所有现有 session round-trip 测试迁移到 serde_json。已完成。
- 新增包含换行、引号、反斜杠、bool、number、null 的事件测试。已完成。
- `--replay` 能读取旧 session 和新 session，或文档明确不兼容边界。
- 删除大部分手写 JSON 字段解析逻辑。已完成。

## TODO-003: ToolDefinition input_schema 升级为结构化 schema

状态：done

来源阶段：v4.1 Read-only Tool Runtime / v4.3 Agent Core 设计校准

当前实现：

- [tool.rs](/Users/jizhishi/mycode/ai/picocode/src/tool.rs) 中 `ToolDefinition.input_schema` 已升级为 `ToolInputSchema`。
- `ToolInputSchema` 可以导出 OpenAI-compatible JSON schema。
- `ToolRuntime::tool_specs()` 可以向 AI 层提供工具定义。

遗留问题：

- 参数校验仍然比较轻量，后续可以继续增强类型错误和默认值处理。
- enum、number range 等高级 schema 还没有实现。

影响范围：

- tool registry
- provider tool schema conversion
- `ls/read`
- 后续 `find/grep/edit/bash`
- 权限和参数校验

补齐时机：

- v4.3 最小 Agent Loop 接入模型工具调用时。
- 或 v5 provider tool schema 转换前必须补齐。

建议方案：

- 定义 `ToolInputSchema`。
- 支持 `string / number / boolean / enum` 等基础类型。
- 支持 `required` 字段。
- `ToolRuntime` 执行前做参数校验。
- provider 层根据 `ToolInputSchema` 转换到 OpenAI / Anthropic / 其他 API 的 tool schema。

验收标准：

- `ls/read` 不再使用字符串 schema。已完成。
- 参数缺失时返回结构化 `ToolResult(status=Error)`。
- schema 能转换成 OpenAI-compatible tool definition。已完成。
- schema 测试覆盖 required、默认值和类型错误。部分完成，后续继续增强。

## TODO-004: 文本工具调用协议升级为 provider-native tool calling

状态：partial

来源阶段：v4.3 最小 Agent Core / v4.4 Provider-native Tool Calling

当前实现：

- [agent_core.rs](/Users/jizhishi/mycode/ai/picocode/src/agent_core.rs) 已优先读取 `AssistantOutput::tool_calls()`。
- [openai.rs](/Users/jizhishi/mycode/ai/picocode/src/ai/openai.rs) 已发送 OpenAI-compatible `tools` 并解析 `tool_calls`。
- [openai.rs](/Users/jizhishi/mycode/ai/picocode/src/ai/openai.rs) 已将 `ToolResultMessage` 序列化为 `role=tool`。
- `<tool_call>...</tool_call>` 文本协议仍作为 fallback 保留。

遗留问题：

- fallback 文本协议还没有删除。
- 手写解析 OpenAI response 的 `tool_calls` 仍然脆弱，后续应随 TODO-002 一起升级到正式 JSON parser。
- 暂不支持 partial tool JSON 和并发 tool call。

影响范围：

- v4.3 Agent Core
- AI provider 层
- ToolRuntime 参数校验
- 后续 `find/grep/edit/bash`
- session 中 tool call id 与 provider tool call id 对齐

补齐时机：

- v5 provider tool schema 转换阶段。
- 或者 MiniMax/OpenAI-compatible API 确认支持 tool calling 后。

建议方案：

- 删除或配置化 `<tool_call>` fallback。
- 使用正式 JSON parser 解析 provider response。
- 支持多个 tool call 的顺序执行或只读并发执行。

验收标准：

- 模型可通过 provider-native tool call 请求 `ls/read`。已完成基础实现。
- 文本 `<tool_call>` 协议被删除或只作为 fallback。当前仍为 fallback。
- tool call id 来自 provider 或由 Agent Core 稳定生成。已完成基础实现。
- provider-native `role=tool` 结果回传。已完成基础实现。
- 工具参数经过 schema 校验。部分完成。
- 测试覆盖 native tool call -> ToolCall event -> ToolResult event -> final answer。已完成。

## TODO-008: 轻量 extension / skill 发现层

状态：open

来源阶段：v14 插件与能力包原则

当前设想：

- 先支持 `~/.picocode/extensions/`、`~/.picocode/skills/`、`<project>/.picocode/extensions/`、`<project>/.picocode/skills/` 这类目录发现。
- extension 先只读 `manifest.toml` 和入口描述。
- skill 先只读 `SKILL.md` 和基础脚本清单。
- 列表与详情都保持 compact，避免引入复杂 marketplace。

遗留问题：

- 还没有 extension 运行时。
- 还没有 skill 的按需加载与上下文注入。
- 还没有统一的 manager UI。

补齐时机：

- v14.1 目录发现
- v14.2 最小 manifest 读取
- v14.3 skill 按需加载
- v14.4 能力详情展开
- v14.5 capability 启用 / 禁用
- v14.6 skill 按需加载与上下文注入

验收标准：

- 能列出全局和项目级能力包。
- 能区分 extension 与 skill。
- 能对单个 capability 打开 compact detail。
- 能对 capability 进行启用 / 禁用切换。
- 能把 skill 按需注入当前上下文。
- 能在不影响主 TUI 的前提下启用少量扩展。
