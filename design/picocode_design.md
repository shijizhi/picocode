# picocode 设计文档

> 版本：v1.0  
> 日期：2026-05-02  
> 目标：基于 Pi Coding Agent 思想，构建一个**功能精简但机制完整**的本地 Coding Agent（TUI-first）

---

## 一、设计原则

- **Small but Real**：功能可以少，但机制必须完整
- **TUI-first**：交互即工作台，不是附加层
- **Event-driven**：所有行为以事件驱动
- **Loop-based**：核心为 Agent Loop（而非单次推理）
- **Tool-first**：通过工具操作真实世界，而非直接生成结果
- **可观察性优先**：用户必须能看清系统行为
- **可演进架构**：每个模块可独立扩展
- **借鉴优先**：交互形态优先参考 Codex CLI、Claude Code、Pi 等成熟 Coding Agent CLI，避免过早发明新范式

---

## 二、总体架构

```
TUI Workbench
    ↓ (Event Stream)
Agent Core (Loop)
    ↓ (Tool Call)
Tool Runtime
    ↓ (Effect)
Workspace (FS / Git / Shell)
```

---

## 三、模块划分

### 1. agent-core

- Agent Loop
- State 管理
- Context 构建
- Stop 控制

### 2. tool-runtime

- 工具注册与调用
- Tool Schema
- 权限控制

### 3. workspace

- 文件系统访问
- Git 状态
- 路径安全控制

### 4. edit-engine

- 文本匹配
- Patch 应用
- Diff 生成
- Rollback

### 5. ai

- 类似 Pi 的 `pi-ai` 接入层
- 统一 `AiClient`
- 统一 `ApiProvider`
- 统一 `Model`
- 统一 `AiContext`
- 统一 `AiMessage`
- 统一 `ContentBlock`
- 统一 `AssistantOutput`
- OpenAI-compatible 接口
- Provider 可扩展

### 6. session-store

- JSONL 事件存储
- Session 恢复

### 7. instruction-loader

- AGENTS.md
- config.toml
- PLAN.md

### 8. TUI Workbench

- 消息流
- 工具时间线
- diff 面板
- 命令输出
- 输入区

---

## 四、核心执行模型

Agent Loop 是 picocode 的核心循环。参考 Pi 的 `pi-agent-core` 设计，picocode 不应该把模型调用、工具执行、事件输出散落在 TUI 或 terminal 模块里，而应该沉淀出独立的 `agent-core`。

Pi 的关键设计启发：

- 低层 loop 只负责推进一轮 agent 执行
- 高层 Agent 持有状态、事件订阅、运行控制
- agent 行为通过事件流向外输出，而不是直接操作 UI
- LLM 输出可以是普通文本，也可以是工具调用
- 工具结果会回填到上下文，驱动下一轮模型调用
- loop 必须有停止条件、中断处理和错误事件
- 内部消息与 LLM 消息要分离，只在模型调用边界做转换
- 用户中途输入不应丢失，应进入 steering / follow-up 队列
- 错误和中断也应该成为可恢复的上下文，而不是只打印日志

横向校准对象：

- Codex：强化 `Submission Queue / Event Queue`、turn lifecycle、agent status、sandbox/approval、context window 管理
- Claude Code：强化 permission modes、hooks、max turns / budget、read-only 工具并发
- OpenCode：强化 provider-agnostic、plan/build agent 分层、typed tool schema、工具输出自动截断、LSP 辅助能力

picocode 的目标结构：

```text
Submission
    ↓
Agent Core
    ↓ emits
Event Stream
    ↓
TUI / Session Store

Agent Core
    ↓ uses
AI Client
Tool Runtime
Workspace
```

最小 Agent Loop：

```
while not done:
    context = build_context(state)
    action = LLM(context)

    if action is tool_call:
        result = execute_tool(action)
        state.append(result)
    else:
        break
```

更准确的阶段模型：

```text
AgentRun
  Turn
    ModelStep
    ToolStep*
    FinalStep
```

### Agent Core 职责

`agent-core` 负责：

- 接收 `Submission`
- 构造 `AiContext`
- 调用 `AiClient`
- 识别 assistant 输出中的 tool call
- 调用 `ToolRuntime`
- 把 `ToolCall / ToolResult / AssistantMessage / Error / Final` 写入事件流
- 判断是否继续下一轮
- 控制最大循环次数
- 处理中断和错误
- 管理 steering / follow-up 队列
- 在调用 LLM 前执行 context transform / convert

`agent-core` 不负责：

- TUI 渲染
- 文件系统细节
- provider-specific API 转换
- 具体工具实现
- session 文件格式

### Agent State

Agent Core 内部维护运行态：

```text
AgentState {
    events
    submissions
    messages
    pending_tool_calls
    steering_queue
    follow_up_queue
    turn_index
    step_index
    max_turns
    max_tool_steps
    token_budget
    cost_budget
    status
}
```

运行状态：

```text
Idle
Running
WaitingForTool
Completed
Failed
Aborted
```

### AgentMessage 与最晚转换

参考 Pi 的 `AgentMessage -> convertToLlm -> Message[]` 思路，picocode 应区分两类消息：

```text
AgentMessage
LLM Message
```

`AgentMessage` 是 agent 内部状态，可以包含：

```text
UserMessage
AssistantMessage
ToolResultMessage
ToolCallEvent
ToolResultEvent
SystemNotice
FileEditEvent
CommandOutputEvent
```

其中只有一部分应该进入模型上下文。转换原则：

- TUI / session / observability 使用完整 `EventMsg`
- Agent Core 内部使用完整 `AgentMessage`
- AI provider 边界才转换为模型能理解的 `AiMessage`
- UI-only 或 audit-only 事件可以被过滤
- 工具结果应转换为 provider-neutral `ToolResultMessage`

当前 v4.2 先把工具事实注入 system prompt，这是可运行的过渡切片。v4.3/v5 应升级为正式的 `ToolResultMessage` 映射。

### Loop 配置钩子

Agent Loop 应保留可插拔钩子，但 v4.3 只实现最小子集：

```text
convert_to_llm
transform_context
get_steering_messages
get_follow_up_messages
before_tool_call
after_tool_call
should_stop_after_turn
prepare_next_turn
```

v4.3 必须优先实现：

```text
convert_to_llm
before_tool_call
after_tool_call
```

其他钩子先保留设计，不急于实现。

### 事件输出原则

Agent Core 的唯一外部输出是事件。参考 Codex 的 SQ/EQ 协议，客户端向 Agent Core 投递 `Submission`，Agent Core 只向外发出 `Event`。Agent 状态应尽量能从事件流派生，而不是依赖 UI 私有状态。

```text
AgentStart
TurnStart
MessageStart
MessageUpdate
MessageEnd
AssistantMessage
ToolCall
ToolExecutionStart
ToolExecutionUpdate
ToolExecutionEnd
ToolResult
Final
Error
TurnEnd
AgentEnd
```

当前 v4.2 已经落地 `ToolCall / ToolResult`。v4.3 不需要一次补齐全部生命周期事件，但模块边界必须允许后续扩展到 `Agent > Turn > Message/Tool` 三层事件。

Codex 的细节值得吸收：

- `Submission` 与 `Event` 是两个异步队列，不应互相阻塞
- `TurnStarted / TurnComplete` 应成为后续一等事件
- `ItemStarted / ItemCompleted` 比单纯文本日志更适合回放和 UI 展示
- `AgentStatus` 可以从事件推导，例如 `Running / Interrupted / Completed / Errored`
- 每个 turn 最终应有 assistant message 或明确的 error/abort 结束态
- context window 管理是 Agent Core 职责，不能完全留给 provider

### Steering 与 Follow-up

Pi 的队列设计值得吸收：

```text
prompt     空闲时开启新任务
continue   空闲时从当前上下文继续
steer      运行中插入用户修正
follow_up  当前任务结束后继续处理
abort      取消当前运行
```

picocode 的阶段策略：

- v4.3：只实现 `prompt` 风格的用户输入
- v5/v6：加入 `continue`
- v10 之前：加入 `abort`
- repair loop 前：加入 `steer / follow_up`

设计原则：

- 用户运行中输入不能悄悄丢弃
- 如果工具链被用户 steering 打断，应产生可见事件
- 被跳过的工具调用应作为错误/跳过结果回填给模型
- follow-up 应在当前 run 正常结束后再进入下一轮

### 错误与恢复

Agent Core 不应该只把错误显示在 UI 里。错误应该进入上下文：

```text
AssistantMessage {
    stop_reason: Error | Aborted
    error_message
}
```

这样用户可以执行 `continue`，模型能看到上一次失败原因并调整策略。v4.3 先保留当前 `Error` event，后续需要把错误也纳入 provider-neutral assistant output。

### 权限、预算与并发

Claude Code 和 Codex 都把“能不能执行工具”作为 agent loop 的一等控制点。picocode 需要保留同样的扩展点：

```text
allowed_tools
denied_tools
permission_mode
max_turns
max_tool_steps
max_budget
```

当前阶段策略：

- `ls/read/find/grep` 默认允许
- 所有 write/execute/network 工具不存在或不可用
- `max_tool_steps = 12`
- `max_model_steps = 16`
- 工具被拒绝时，应以 `ToolResult(status=Denied)` 回填给模型，而不是静默失败

后续策略：

- read-only 工具可以并发执行
- write / edit / bash 必须串行执行
- hooks 和 permission checks 必须在工具执行前运行
- deny 规则优先级高于 allow 和自动模式
- budget / max turns 命中时应产生明确终止事件

### v4.3 设计调整

v4.3 的目标不是做完整 Pi Agent Core，而是做一个“长得像 Pi 的最小核心”：

```text
AgentCore::submit(submission)
AgentCore::run_once()
AgentCore emits Vec<Event>
```

内部先支持：

```text
UserInput
ModelStep
Optional ToolStep(ls/read)
Final ModelStep
```

并明确保留后续扩展点：

```text
convert_to_llm
tool preflight
tool finalize
turn lifecycle
abort signal
steering queue
follow-up queue
```

### v4.3 吸收范围

v4.3 不直接实现完整 Agent Core，而是实现最小可运行切片：

- 新增 `agent` 或 `agent_core` 模块
- 从 `terminal` 中移出“收到用户输入后调用 AI”的逻辑
- `AgentCore::submit_user_input`
- `AgentCore::run_once`
- 支持一次模型回答
- 支持模型请求 `ls/read`
- 执行工具后最多再调用一次模型生成最终回答
- 所有行为通过 `EventMsg` 输出

v4.3 暂不做：

- streaming
- 多工具并发
- approval
- edit/write/bash
- long-running task orchestration
- 完整 turn lifecycle event

当前原则：

> terminal 只负责 UI 事件循环；Agent Core 负责 agent 生命周期；Tool Runtime 负责工具执行；AI 层负责模型适配。

---

## 五、演进路径（v0 → v12）

演进主线不是零散学习知识点，而是让 picocode 逐步长成一个完整产品。

每个阶段必须满足三个条件：

1. 有明确产品能力
2. 有可运行演示
3. 有一个聚焦学习主题

学习是阶段交付的附加价值，而不是阶段本身的目标。

阶段推进中刻意留下的简化实现和技术债，必须记录到 [TODO / 技术债](./todo.md)，不能只停留在对话里。

### v0: TUI Shell

产品产物：

- 一个可以启动的 `picocode` TUI 程序

能力范围：

- 启动 TUI
- Transcript 消息流
- 底部输入区（composer）
- 底部状态行
- 用户输入后显示在消息流里
- 支持退出
- 暂不接入 LLM

验收演示：

```text
$ picocode
> hello
User: hello
```

学习主题：

- TUI 应用结构
- 事件循环
- 终端交互基础

### v1: Event Stream

产品产物：

- 用户输入和系统输出进入 Codex 风格的结构化协议

能力范围：

- 定义基础 `Submission / Op`
- 定义基础 `Event / EventMsg`
- `UserMessage`
- `AssistantMessage`
- `Error`
- `Final`
- TUI 基于事件渲染消息
- 内存中维护 submission list 和 event list

验收演示：

```json
{"id":"sub-0","op":{"type":"UserInput","content":"hello"}}
{"type":"UserMessage","content":"hello"}
{"type":"AssistantMessage","content":"..."}
```

学习主题：

- Event-driven architecture
- 事件作为系统内部契约

### v2: LLM Chat

产品产物：

- 一个能和模型对话的本地 TUI chat

能力范围：

- 接入 OpenAI-compatible API
- 建立类似 Pi `pi-ai` 的 AI provider 接入层
- 终端和 Agent Core 只依赖统一 `AiClient / ApiProvider`
- 使用 `Model + AiContext + AiMessage + ContentBlock + AssistantOutput` 作为 AI 层核心契约
- 支持配置 provider / model / api key
- 基于事件流构造 prompt / context
- 显示 assistant 回复
- 支持基础错误展示
- 首版通过 `PICOCODE_AI_PROVIDER`、`OPENAI_API_KEY`、`OPENAI_BASE_URL`、`PICOCODE_MODEL` / `OPENAI_MODEL` 配置

验收演示：

```text
User: 解释一下这个项目是做什么的
Assistant: 当前还没有项目读取能力，但我可以回答通用问题...
```

学习主题：

- LLM provider 抽象
- Prompt 构造
- Streaming 输出

### v3: Session Store

产品产物：

- 对话可以保存、恢复、回放

能力范围：

- JSONL 保存事件
- 使用 Codex-style rollout line：`timestamp + type + payload`
- 首行写入 `session_meta`
- 后续事件写入 `event_msg`
- 每次启动创建 session
- 支持列出历史 session
- 支持恢复指定 session
- 支持只读 replay
- 事件和 session 格式带版本字段
- session 默认保存到 `.picocode/sessions/<session-id>.jsonl`

落盘格式示例：

```jsonl
{"timestamp":"1778499710023","type":"session_meta","payload":{"format_version":1,"session_id":"session-1778499710023","cwd":".","app_version":"0.1.0"}}
{"timestamp":"1778499710123","type":"event_msg","payload":{"id":"evt-0","type":"user_message","content":"hello"}}
```

验收演示：

```text
picocode --list-sessions
picocode --resume <session-id>
picocode --replay <session-id>
```

学习主题：

- Session 作为一等对象
- Agent 工作过程的持久化和回放

### v4: Read-only Tool Runtime

产品产物：

- picocode 拥有第一版工具系统，并能通过只读工具理解当前项目的基本结构

能力范围：

- 定义基础 `ToolDefinition / ToolCall / ToolResult`
- 定义工具权限等级：read / write / execute
- 定义工具结果截断协议
- 定义工具调用事件和结果事件
- workspace root 识别
- 路径解析和路径安全限制
- `ls`：列出目录
- `read`：读取文件，支持 offset / limit
- respect `.gitignore`
- TUI 显示工具调用和工具结果
- AI 可把工具结果纳入下一轮 context

初始工具：

```text
ls
read
```

验收演示：

```text
User: 看一下这个项目结构
Assistant: 通过 ls / read 读取项目后，总结项目结构。
```

学习主题：

- Workspace abstraction
- Tool contract
- 路径安全
- Read-only agent

### v5: Tool Runtime

产品产物：

- 工具系统从只读工具扩展到可搜索、可扩展的工具运行时

能力范围：

- tool registry
- tool schema
- provider 侧 tool schema 转换
- tool call partial JSON 拼接
- 工具错误结构化返回
- TUI 展示工具调用时间线
- 引入搜索类只读工具

事件扩展：

```text
ToolCall
ToolResult
```

初始新增工具：

```text
find
grep
```

验收演示：

```text
User: README 里有没有安装说明？
Agent:
1. 调用 find / ls
2. 找到 README
3. 调用 read
4. 总结结果
```

学习主题：

- Tool-first agent design
- 工具调用协议

### v6: Instruction Loader

产品产物：

- picocode 能读取项目级指令并影响后续行为

能力范围：

- 加载 `AGENTS.md`
- 加载 `picocode.toml`
- 可选加载 `PLAN.md`
- 指令注入 context
- TUI 显示已加载 instruction 来源
- 明确 instruction 优先级和截断策略

验收演示：

```md
# AGENTS.md
回答请使用中文。
修改代码前先说明计划。
```

Agent 后续回复和行动应受项目指令影响。

学习主题：

- Context engineering
- 项目指令和运行时上下文控制

### v7: Search Agent

产品产物：

- picocode 可以在项目中搜索并定位相关代码，形成 Pi 风格的只读探索层

能力范围：

- `find_files`
- `search_text`
- 旧名 `find / grep` 作为兼容别名保留
- 搜索结果截断
- 搜索结果排序
- 防止一次塞入过多 context
- TUI 展示搜索命中

验收演示：

```text
User: 找到处理 CLI 参数的代码
Assistant: 搜索 args / parse / clap 等相关位置，并总结判断。
```

学习主题：

- 代码库导航
- 上下文选择
- 搜索结果压缩
- Pi 风格 read-only exploration

### v8: Edit Preview

产品产物：

- picocode 可以生成修改方案和 diff 预览，但不直接写文件

能力范围：

- `propose_edit`
- unified diff / patch preview
- TUI diff 面板
- 失败时显示原因
- 只读，不修改 workspace

初始工具：

```text
propose_edit
```

验收演示：

```text
User: 给 CLI 增加 --version 参数
Assistant: 提出具体文件修改，并展示 diff。
```

学习主题：

- Edit safety
- 从“生成代码”到“可审查修改”
- Pi / Codex 式 preview-first edit flow

### v9: Edit Engine

产品产物：

- picocode 能稳定、安全地修改文件，并在写入前后保留可审查轨迹

能力范围：

- 精确匹配
- 预览与应用分离
- patch apply
- 多文件 batch edit
- 写入前冲突检测
- 修改前锚点校验
- edit checkpoint / rewind
- 批量编辑支持基于 `base_hash` / `expected_hash` 的乐观冲突检测
- 修改结果以 checkpoint 事件写入 session，可回放、可恢复
- 中断后可通过 session resume 继续未完成思路，或对最近一次编辑执行 rewind
- 回退依赖 session 里的 checkpoint 链和锚点校验，而不是单独的备份目录
- `FileEdit` 事件、批量编辑确认面板和更完整冲突解释继续补齐

验收演示：

```text
modified src/main.rs
+ ...
- ...
```

失败时应能说明：

```text
patch failed: target block not found
```

学习主题：

- Deterministic editing
- 可回滚修改
- Pi / Codex 式 preview-first 或 auto-apply 流程
- 批量编辑与冲突收敛

### v10: Command Agent

产品产物：

- picocode 能运行命令，例如测试、构建、格式化

能力范围：

- `run_command`
- timeout
- stdout / stderr 捕获
- exit code
- risky command approval
- command allow / deny policy
- TUI 实时展示命令输出

事件扩展：

```text
CommandRun
CommandOutput
```

验收演示：

```text
User: 运行测试
Agent: 执行 cargo test，并总结成功或失败原因。
```

学习主题：

- 执行环境
- 权限边界
- 命令结果结构化

### v11: Repair Loop

产品产物：

- picocode 能根据测试 / 构建错误自动修复

能力范围：

- 运行命令
- 捕获失败
- 将错误摘要放回 context
- 搜索相关代码
- 修改代码
- 再次运行命令
- 设置最大循环次数
- 防止无限修复

验收演示：

```text
User: 修复当前测试失败
Agent:
1. 跑测试
2. 读错误
3. 找代码
4. 修改
5. 再跑测试
6. 输出最终结果
```

学习主题：

- Feedback loop
- 从“能改代码”到“能完成任务”

### v12: TUI Workbench

产品产物：

- 一个完整可用的 picocode workbench

能力范围：

- 消息流
- 工具时间线
- diff 面板
- 命令输出面板
- session 列表
- session resume
- session replay
- final summary
- 配置文件
- 基础文档

最终验收演示：

```text
User: 为这个 Rust CLI 增加 --version 参数，并补充测试
```

系统应能：

1. 理解项目结构
2. 搜索 CLI 参数代码
3. 修改代码
4. 修改测试
5. 运行测试
6. 根据错误修复
7. 展示 diff
8. 保存 session
9. 输出总结

学习主题：

- 将 Agent Loop、Tool System、Edit Engine、Feedback Loop 和 Observability 组合成产品

---

## 六、核心能力说明

### 1. Context Engineering
- 文件选择
- 内容截断
- 优先级排序

### 2. Tool System
- JSON 调用协议
- 可扩展工具
- 错误反馈

### 3. Edit Engine（关键）
- 精确匹配
- 模糊匹配
- Diff 输出
- Rollback

### 4. Execution Loop
- run_command
- 错误捕获
- 自动修复循环

### 5. Observability
- 全量事件流
- TUI 实时展示
- Session 回放

---

## 七、AI 接入层与 Pi 差距

picocode 的 `ai` 模块应持续向 Pi 的 `pi-ai` 靠拢。当前 v2 已经完成最小骨架：

- `AiClient`
- `ApiProvider`
- `Model`
- `ModelCapabilities`
- `AiContext`
- `AiMessage`
- `ContentBlock`
- `AssistantOutput`
- OpenAI-compatible provider

但当前实现仍只是 Pi `pi-ai` 的一个很小子集，后续必须补齐以下差距。

### 1. Streaming 事件

Pi `pi-ai` 的核心是流式事件，而不是一次性 `complete -> AssistantOutput`。

后续需要补：

- `AiStreamEvent`
- `text_start`
- `text_delta`
- `text_end`
- `thinking_start`
- `thinking_delta`
- `thinking_end`
- `tool_call_start`
- `tool_call_delta`
- `tool_call_end`
- `done`
- `error`

TUI 应能边接收边渲染 assistant 输出。

### 2. Tool Calling

Pi `pi-ai` 把工具定义、工具调用、工具结果都作为 AI 上下文的一等对象。

后续需要补：

- `ToolSpec`
- JSON schema 输入定义
- `ToolCallContent`
- `ToolResultMessage`
- provider 侧 tool schema 转换
- tool call partial JSON 流式拼接
- tool use stop reason

### 3. Rich Content Blocks

当前 `ContentBlock` 只真正使用 `Text`，`Thinking` 和 `ToolCall` 只是预留。

后续需要补：

- Thinking content 的真实 provider 映射
- Image content
- File / attachment content
- 多 content block 的顺序和索引
- content block 与 TUI transcript block 的映射

### 4. Model Registry

Pi `pi-ai` 有模型注册和模型元数据，而当前只是从环境变量读一个 model id。

后续需要补：

- model registry
- provider / api / model 三者分离
- context window
- max output tokens
- capability flags
- 默认模型选择
- provider-specific model alias

### 5. API Provider Registry

Pi 区分 provider 与 api。同一个 provider 可能有多种 API，同一个 API 也可能被不同服务兼容。

后续需要补：

- api provider registry
- provider lazy loading
- `openai-chat-completions`
- `openai-responses`
- `anthropic-messages`
- `google-generative-ai`
- `openrouter`
- local / custom OpenAI-compatible endpoints

### 6. Usage / Cost / Diagnostics

Pi `pi-ai` 会追踪 usage、cost、diagnostics。

后续需要补：

- input tokens
- output tokens
- cached tokens
- reasoning tokens
- cost estimate
- response id
- provider diagnostics
- finish / stop reason mapping

### 7. Error / Abort / Partial Output

当前错误只是字符串事件。Pi 的设计需要把错误、中断、部分输出都纳入 assistant 输出和 stream event。

后续需要补：

- abort signal
- partial assistant output
- provider error normalization
- retryable / non-retryable 分类
- rate limit 错误识别
- auth / quota / network 错误识别

### 8. Context Handoff

Pi `pi-ai` 支持跨模型、跨 provider 的 context handoff。

后续需要补：

- provider-neutral context serialization
- provider-specific message conversion
- unsupported content downgrade
- tool result conversion
- image/content compatibility handling

当前原则：

> v2 可以先只做 `complete`，但类型边界必须面向 Pi `pi-ai` 的 streaming、tool calling、rich content 和 model registry 演进。

---

## 八、事件模型

事件模型优先参考 Codex 的协议分层：

- `Submission / Op`：外部输入给 Agent Core 的操作请求（后续 Agent Loop 阶段引入）
- `Event / EventMsg`：Agent Core 输出给 TUI / Session Store / 外部消费者的事件流

当前 v1 先落地输出侧事件流：

```rust
Event {
    id: EventId,
    msg: EventMsg,
}
```

`EventMsg` 是具体事件负载：

```text
SystemMessage
UserMessage
AssistantMessage
ToolCall
ToolResult
FileRead
FileEdit
CommandRun
CommandOutput
Error
Final
```

这样设计的原因：

1. TUI 渲染的是 `Event`，而不是临时消息列表
2. Session Store 可以按 JSONL 一行一个 `Event` 保存
3. Tool Runtime / Edit Engine / Command Agent 都能把行为写入同一条事件流
4. 后续接入 Agent Loop 时，可以自然形成 `Submission Queue -> Agent Core -> Event Stream`

---

## 九、工具系统设计

工具系统是 v4 之后的核心地基。工具选择需要综合 Pi、Codex CLI、Claude Code 的共同设计，而不是只复制某一个项目。

### 1. 参考对象

#### Pi

Pi 的 coding agent 默认围绕工具工作，而不是一次性把 workspace 塞进 prompt。它的关键启发：

- 内建 `read / write / edit / bash`，后续扩展 `ls / find / grep`
- `read` 支持 `path / offset / limit`
- `ls` 只列目录，并限制返回数量和字节数
- `find` / `grep` 负责代码定位，读取仍交给 `read`
- 工具有 `renderCall / renderResult`，天然服务 TUI 可观察性
- 文件操作背后抽象为 operations，未来可以替换成本地、容器、远程环境

#### Claude Code

Claude Code 的工具选择更完整，适合我们做工具分层参考：

- 只读工具：`Read / LS / Glob / Grep`
- 写入工具：`Edit / MultiEdit / Write`
- 执行工具：`Bash`
- 编排工具：`Task`
- 工具权限可配置，危险操作需要确认或被策略拦截

它的启发是：工具应该按风险分层，而不是平铺成一堆函数。

#### Codex CLI

Codex CLI 的启发主要在安全和可审计执行：

- 文件编辑通过 patch/diff 形式落地
- shell 执行受 sandbox 和 approval policy 约束
- agent 行为要能被 transcript / session 记录
- 用户应该能看到将要执行的高风险动作

它的启发是：工具运行时必须从第一天就考虑权限、审计和回放。

#### OpenCode

OpenCode 的启发主要在工具工程化和 agent 分层：

- 工具定义应有明确 schema，而不是靠自然语言拼参数
- 工具结果应自动截断，防止一次输出污染上下文
- read 工具应支持 offset / limit，并尽量按行或稳定边界读取
- plan agent 与 build agent 可以有不同工具权限
- LSP / diagnostics 可以作为后续代码理解能力，但不进入 v4 首批

它的启发是：工具不仅是函数，还要服务上下文预算、权限边界和 agent 角色差异。

### 2. 工具分层

picocode 的工具分为四层：

```text
Read Tools
Write Tools
Execute Tools
Meta Tools
```

#### Read Tools

只读取 workspace，不产生副作用。

首批：

```text
ls
read
```

后续：

```text
find
grep
```

设计原则：

- 默认允许
- 必须限制在 workspace root 内
- 必须支持结果截断
- 大文件必须支持 offset / limit 继续读取
- 读取结果必须进入事件流

#### Write Tools

修改 workspace 文件。

后续工具：

```text
edit
multi_edit
write
apply_patch
```

设计原则：

- 默认需要用户确认
- 优先 diff / patch，而不是静默写文件
- 每次修改必须产生可回放事件
- 修改前应保留快照或可逆信息

#### Execute Tools

运行命令，影响真实环境。

后续工具：

```text
bash
run_command
```

设计原则：

- 默认需要权限策略判断
- 捕获 stdout / stderr / exit code
- 必须有 timeout
- 高风险命令必须确认
- 输出必须可截断、可继续显示

#### Meta Tools

帮助 agent 规划、委派或管理上下文。

后续工具：

```text
task
plan
todo
```

设计原则：

- 不直接操作 workspace
- 主要服务长任务分解和上下文压缩
- 不进入 v4 首批范围

### 3. v4 首批工具选择

v4 只做 read-only tool runtime，首批工具是：

```text
ls
read
```

暂不做：

```text
write
edit
bash
grep
find
task
```

原因：

1. `ls/read` 是最小闭环，足够让 agent 理解项目结构
2. read-only 风险最低，适合先打磨工具协议和 TUI 可观察性
3. `grep/find` 更偏代码导航，放到 v5/v7 可以更聚焦
4. `edit/bash` 涉及权限、回滚、审批，必须等工具事件和 session 稳定后再做

### 4. 核心类型

```rust
ToolDefinition {
    name,
    description,
    input_schema,
    permission,
}

ToolCall {
    id,
    name,
    arguments,
}

ToolResult {
    call_id,
    status,
    content,
    truncated,
    next_offset,
}
```

参考 OpenCode 和 Claude Code，`input_schema` 使用正式 schema 类型，而不是字符串：

```rust
ToolInputSchema {
    properties,
    required,
}
```

v4.4 已落地 `ToolInputSchema -> ToolSpec.input_schema_json`，OpenAI-compatible provider 可以把它转换成 native tools schema。

权限等级：

```text
Read
Write
Execute
Network
```

结果状态：

```text
Success
Error
Denied
Truncated
```

### 5. 事件映射

工具不直接写 UI，工具只产生事件。

```text
ToolCall
ToolResult
```

后续可以根据需要派生更细事件：

```text
FileRead
FileEdit
CommandRun
CommandOutput
```

但早期不应过度拆分。v4 先统一用 `ToolCall / ToolResult`，TUI 再根据工具名渲染。

### 6. TUI 展示原则

参考 Pi 的 `renderCall / renderResult`，但不要把渲染逻辑塞进工具本身。

picocode 的设计：

- tool-runtime 只返回结构化结果
- TUI 根据 `ToolCall / ToolResult` 渲染简洁时间线
- transcript 中显示工具名、参数摘要、结果摘要
- 大块文件内容默认折叠或截断
- session 中保存完整结构化事件

### 7. 路径安全

v4 必须内建路径安全：

- 所有相对路径基于 workspace root
- 支持用户输入 `./src/main.rs`
- 可考虑支持 Pi 风格 `@src/main.rs`
- 禁止 `..` 逃逸 workspace root
- 禁止读取 `.git/` 内部实现文件
- 遵守 `.gitignore`
- 默认跳过二进制文件和超大文件

### 8. 结果截断协议

工具结果不能无限进入 context。

`read`：

```text
path
offset
limit
content
truncated
next_offset
```

`ls`：

```text
path
entries
truncated
entry_limit
```

截断不是错误，而是正常状态。模型应该能根据 `next_offset` 决定是否继续读取。

### 9. 实施顺序

v4.1：

- `tool` 模块
- `workspace` 模块
- `ToolDefinition / ToolCall / ToolResult`
- `PermissionKind`
- `ls`
- `read`

v4.2：

- `ToolCall / ToolResult` 事件
- TUI 工具时间线
- session 持久化工具事件

v4.3：

- 新增最小 `agent-core`
- 将用户输入后的 AI 调用逻辑从 `terminal` 移出
- `Submission -> AgentCore -> Event Stream`
- 模型可请求 `ls/read`
- 执行工具并产生 `ToolCall / ToolResult`
- 工具结果回填 context
- 最多二次模型调用，生成最终回答

v4.4：

- `ToolDefinition.input_schema` 升级为 `ToolInputSchema`
- `ToolRuntime::tool_specs()` 导出 provider-neutral `ToolSpec`
- `AiContext.tools` 正式携带工具定义
- OpenAI-compatible provider 发送 `tools` / `tool_choice=auto`
- OpenAI-compatible provider 解析 `tool_calls`
- `AssistantOutput::tool_calls()` 暴露 native tool call
- Agent Core 优先执行 native tool call
- `<tool_call>` 文本协议仅作为 fallback

v4.5：

- `EventMsg::ToolCall` 转换为 assistant `tool_calls` message
- `EventMsg::ToolResult` 转换为 provider-neutral `ToolResultMessage`
- OpenAI-compatible provider 将 `ToolResultMessage` 序列化为 `role=tool`
- 第二次模型调用通过 native tool result 上下文生成最终回答

v5.0：

- 引入 `serde_json`
- Session JSONL 生成 / 解析从手写字符串切换到正式 JSON parser
- OpenAI-compatible `tool_calls` response 解析切换到 `serde_json`
- 补充包含引号、反斜杠、换行、bool、number、null 的 round-trip 测试
- 删除旧的手写 chat content JSON parser

v5.1：

- 新增 `find` 只读工具
- `Workspace::find(query, path, limit)` 递归发现 workspace 内文件和目录
- `find` 使用 workspace-relative path 的大小写不敏感子串匹配
- `find` 复用现有 ignore 规则，跳过 `.git/`、`target/`、`.picocode/` 和基础 `.gitignore` 匹配项
- `find` 结果按路径稳定排序，并支持 `limit` 截断
- CLI 支持 `picocode --tool find <query> [path limit]`
- Agent 可通过 provider-native tool schema 看到 `find`，用于先定位候选文件，再调用 `read` 精读

v5.2：

- 新增 `grep` 只读工具
- `Workspace::grep(query, path, limit, ignore_case)` 在 workspace 文件内容中搜索 literal 文本
- `grep` 可从文件或目录开始搜索，目录搜索会递归进行
- `grep` 返回 `path:line:content`，用于让 Agent 精确定位命中位置
- `grep` 跳过 ignored 路径、二进制文件和非 UTF-8 文件
- `grep` 支持 `limit` 截断和 `ignore_case`
- CLI 支持 `picocode --tool grep <query> [path limit ignore_case]`
- Agent 可先用 `find` 缩小候选，再用 `grep` 定位文本命中，最后用 `read` 精读上下文

v5.3：

- Agent Core 从“一次工具调用 + 最终回答”升级为多步工具循环
- 默认最多 `16` 次 model step、`12` 次 tool step，允许常见的多轮检索和读取，同时防止无限循环
- 每轮模型输出如果是 tool call，就执行工具并把 `ToolCall / ToolResult` 回填上下文
- 每轮模型输出如果不是 tool call，就记录 `AssistantMessage` 并结束
- 当前支持 `find -> grep -> read -> answer` 这种渐进式代码定位流程
- provider-native tool calling 和文本 fallback 都走同一套循环
- native JSON tool arguments 支持 `query / path / offset / limit / ignore_case` 等搜索参数

v5.4：

- OpenAI-compatible provider 不再把 API 错误压扁为 `EmptyResponse`
- `curl` 响应会携带 HTTP status，先判断 `4xx/5xx` 再解析 chat message
- provider 返回 `{"error": ...}` 时，会向用户展示真实 `message / type / code`
- provider 返回非预期 JSON 时，会展示响应 body preview，方便定位服务端兼容性问题
- 目标是让用户能看到可处理的错误，例如鉴权失败、模型不支持 tools、额度不足、请求格式错误

v5.5：

- 修复 provider-native tool result 回填时的 `function.arguments` 格式
- 内部 `ToolRuntime` 继续使用 `key=value` 参数格式，便于当前工具执行
- OpenAI-compatible provider 在序列化 assistant `tool_calls` 时，将 `key=value` 转回合法 JSON string
- 修复服务端报错：`invalid function arguments json string`
- 覆盖 `query/path/limit/ignore_case` 等搜索工具参数

v5.6：

- TUI transcript 从简单 `label + content` 升级为 agent timeline
- 用户消息使用独立块展示，增强当前问题的可见性
- 工具调用从协议格式升级为语义动作，例如 `# Search text` / `$ grep ...`
- 工具结果显示 `status / truncated`，内容使用左侧竖线缩进
- 多行 `grep/read` 输出保持逐行展示，避免挤成一整行
- footer 更新为 `picocode · status · tools · stage v5.6`
- 交互方向参考 opencode：过程可观察，但最终回答仍保持独立清晰

v5.7：

- 参考 Pi 的工具结果展示策略，对 TUI 中的 tool result 做默认折叠
- 折叠仅影响 UI 展示，不改变 session 保存和模型上下文中的完整工具结果
- `grep` 默认展示前 `15` 行，剩余行显示 `... N more lines (collapsed)`
- `find` 默认展示前 `20` 行
- `read` 默认展示前 `40` 行
- 未知工具结果默认展示前 `20` 行
- 保留 `truncated` 状态展示，用于区分“工具层截断”和“UI 层折叠”
- 后续可在此基础上增加选中 tool result 后展开

v5.8：

- 工具步数上限属于 agent 内部预算约束，不作为用户可见 error 展示
- 当 `max_tool_steps` 用尽但模型仍请求工具时，停止继续执行工具
- Agent Core 会向最终模型调用注入内部 system notice，要求基于已有证据收尾
- 最终回答调用不携带 tools schema，避免模型继续请求工具
- 用户只看到已有工具时间线和最终 assistant answer，不暴露内部预算细节

v5.9：

- TUI 视觉主题切换到 Dracula-inspired palette
- 使用 Dracula 官方语义色：background/current line/foreground/comment/cyan/green/orange/pink/purple/red/yellow
- 用户输入使用 green，assistant 正文使用 foreground，tool action 使用 purple/pink
- tool command 和 tool result 正文使用 subtle 灰色，弱信息和竖线使用 comment
- error 使用 red，truncated/warning 使用 orange
- footer 和 editor border 使用 current line，形成更统一的终端主题感
- 后续调色：最终回答降为 answer 灰色，过程层输出进一步降为 process 灰色，降低长文本亮度疲劳

v5.10：

- 在进入 v6 Instruction Loader 前，先补齐项目目录边界
- CLI 支持 `--project <path>` 指定 workspace root
- TUI、ToolRuntime、session store、后台 Agent Core 都使用同一个 project root
- 默认仍为当前目录，保持已有使用方式兼容
- RustRover 调试时可在 Run Configuration 中传入 `--project /path/to/target/project`
- 目标是避免开发 picocode 时误把 picocode 自身仓库当成被操作项目

v5.11：

- 参考 Codex 的反馈方式，增加长任务运行态心跳
- `AppState` 引入 `RuntimeStatus`，用于描述当前运行阶段
- footer 直接显示 `status + elapsed + detail`，例如 `thinking 12s running agent loop`
- 运行态主要用于 agent 执行阶段，不把同步的 AI 配置加载停留成长期 `loading`
- 运行态与 `pending_ai_requests` 分离，避免 UI 只显示模糊的 busy/idle
- 目标是让长任务期间不再“像卡住了”

v5.12：

- 取消顶部状态横幅，避免在内容区上方再创造一个独立视觉层
- 状态反馈统一收敛到 footer，遵循 Codex 式克制展示
- 内容区只保留用户消息、工具调用、工具结果和最终回答
- 继续保留运行态，但不再以额外 banner 形式呈现

v6.1：

- 新增 `InstructionLoader`，按 Pi 的方式加载上下文文件
- 从 `PICOCODE_HOME` 加载全局 `AGENTS.md` / `CLAUDE.md`
- 从项目根到当前目录链路依次加载 `AGENTS.md` / `CLAUDE.md`
- 同时加载项目根的 `picocode.toml` / `PLAN.md`
- 启动时将加载到的 instruction source 注入 system context，并在 TUI 中可见
- session resume 时跳过重复注入，避免上下文膨胀
- 当前先按 raw text 注入，后续再做 `picocode.toml` 语义化解析

v6.2：

- `picocode.toml` 从文本上下文升级为项目配置入口
- 先支持 `workspace.respect_gitignore`
- 该配置会影响 workspace / tool runtime 是否读取项目内 `.gitignore`
- `picocode.toml` 仍会以一条简洁的 project config 摘要出现在上下文中，便于模型知道项目规则
- 后续再扩展 `command.approval`、`command.timeout` 等运行参数

v6.3：

- `picocode.toml` 中的 `command.approval` 从字符串升级为结构化枚举策略
- `command.timeout` 保持结构化数值，后续给 command agent / shell runner 直接使用
- 非法的 `command.approval` 值会被记录为 warning，避免静默降级
- 当前阶段先把 command policy 解析和摘要固化下来，不立即接命令执行本体
- 这样后面进入 `v10 Command Agent` 时可以直接消费策略对象，而不是再补一层字符串解析

v6.4：

- instruction source 增加显式来源标签，区分 `global / project / config / plan`
- 固定加载顺序为：全局 `PICOCODE_HOME`，再项目目录链路，最后项目配置和计划文件
- resume 时识别统一的 instruction 注入前缀，避免重复注入
- 这一步先把“上下文来源与优先级”收口，后面再进入 `search/edit/command` 时就不会混淆来源

v6.5：

- 启动时先显示一条 instruction 总览摘要，再展开具体来源明细
- 摘要只负责让用户快速确认“当前项目规则已加载”，不引入新的顶部 banner
- resume 时同样识别摘要前缀，避免重复注入
- 目标是把 instruction 加载从“多条零散系统消息”收敛成“一个总览 + 若干来源明细”

v7：

- 搜索层对齐 Pi 风格的只读探索入口，主工具名改为 `find_files` / `search_text`
- 保留 `find / grep` 作为兼容别名，降低迁移成本
- `find_files` 增加相关性排序，优先返回更像候选目标的路径
- `search_text` 继续返回行号命中，用于后续 `read` 精读
- CLI 和 TUI 统一使用新的搜索术语，便于后续 Search Agent 扩展

v5：

- registry 完整化
- `find / grep`
- 更完整的错误和截断策略

当前原则：

> v5 不追求一次性做完“搜索智能”，而是先把可控、可观察、可截断的检索工具放入 Agent Loop。`find` 负责发现候选路径，`grep` 负责定位文本命中，`read` 负责精读内容，Agent Core 负责把这些工具串成多步推理循环。

---

## 十、配置设计（TOML）

picocode 不以环境变量作为长期配置方式。参考 Pi 的 auth/model 配置思路，但使用单一 TOML 文件，避免用户在多个文件之间查找配置：

- `~/.picocode/config.toml`：选择当前 provider / model、配置凭证来源、配置 provider / API / models / capabilities

配置解析必须使用正式 TOML parser，不使用手写临时 parser。

### ~/.picocode/config.toml

```toml
[model]
provider = "openai"
model = "gpt-5"

[auth.openai]
type = "api_key"
key = "OPENAI_API_KEY"

[providers.openai]
base_url = "https://api.openai.com/v1"
api = "openai-chat-completions"
auth = "openai"

[[providers.openai.models]]
id = "gpt-5"
tools = false
images = false
reasoning = false
```

`auth.*.key` 支持三种形式：

```text
sk-...        明文 key
OPENAI_API_KEY 从环境变量读取
!command      执行命令读取 stdout，例如 macOS Keychain / 1Password CLI
```

项目级配置后续可使用仓库内 `picocode.toml`：

```toml
[workspace]
root = "./"
respect_gitignore = true

[command]
approval = "on_risky"
timeout = 120
```

---

## 十一、最终目标能力

用户输入：

> 为这个 Rust CLI 增加 --version 参数，并补充测试

系统应能：

1. 理解项目结构
2. 搜索相关代码
3. 修改代码
4. 修改测试
5. 运行测试
6. 自动修复错误
7. 展示 diff
8. 输出总结

---

## 十二、总结

picocode 的本质：

> 一个基于 Agent Loop、Tool System 和 Feedback Loop 的可观察 Coding Agent Workbench

不是代码生成工具，而是：

> 自动化程序员系统（Automated Programmer System）

---

## 十三、v14 插件与能力包原则

参考 Pi 的方向，picocode 的插件系统不追求“大而全的扩展平台”，而是先做一个轻量、可发现、可启用、可组合的能力层。核心原则如下：

- **先轻后重**：先做目录发现和静态描述，再做最小 API，最后才考虑管理 UI
- **能力分层**：
  - `extension` 负责代码行为，例如注册工具、命令、事件
  - `skill` 负责任务知识，例如指令、参考资料、脚本
  - `manager` 负责发现、启用、禁用、浏览
- **默认克制**：列表 compact，详情按需展开，不先做复杂 marketplace
- **本地优先**：先支持项目级和全局级目录，先不做远程安装和版本协商
- **UI 边界清晰**：插件可以影响状态和少量交互，但不能接管整套 TUI
- **借鉴 Pi 的简洁感**：发现机制简单，入口少，用户能一眼看懂当前可用能力

建议的最小目录结构：

```text
~/.picocode/
  extensions/
  skills/

<project>/.picocode/
  extensions/
  skills/
```

建议的最小能力包定义：

```text
extension/
  manifest.toml
  main.rs

skill/
  SKILL.md
  scripts/
  references/
```

下一阶段不直接做复杂 marketplace，而是先验证：

1. 目录发现是否足够清晰
2. 能力描述是否足够 compact
3. 是否能在不破坏主 UI 的情况下启用少量扩展
4. `skill` 是否能像 Pi 一样作为轻量知识包被按需读取

### 5. v14.4 能力详情展开

在保持能力列表 compact 的前提下，为单个 capability 提供按需详情查看入口，避免引入复杂管理页面。

目标是：

- `/capabilities` 继续只看摘要列表
- `/capability <query>` 进入单条 capability 详情
- detail 里只展示必要的元信息和有限预览
- 多条匹配时仍保持 compact 列表，不做新面板

### 6. v14.5 Capability 启用 / 禁用

在不增加复杂管理页的前提下，提供最小的 capability 开关能力：

- 列表只显示已启用 capability
- 详情可查看某个 capability 的启用状态
- 通过轻量本地配置文件记录 project 级启用 / 禁用状态
- 保持 Pi 式的 discover-first 体验，不引入重插件平台

### 7. v14.6 Skill 按需加载

借鉴 Pi 的 skill 设计，skill 不是重插件，而是可以按需读入上下文的轻量知识包。

目标是：

- `/skill <query>` 直接把匹配的 `SKILL.md` 注入当前会话上下文
- 注入内容保持原文，避免再造技能执行层
- 仅在 capability 被启用时允许加载
- 继续复用 event -> context 的现有链路，不增加新的运行态
