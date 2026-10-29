# omp-desktop UI 设计规范（GPUI + gpui-kit 0.7）

> **设计方向：Zed 级工业精密工作台（Industrial Precision Workbench）**  
> **理由**：面向每日连续数小时盯着 coding agent 并发工作的工程师，界面摒弃营销装饰与柔和悬浮卡片，采用 1px 结构体分割、固定等宽行高、零布局抖动（Zero-Reflow）的流式增量容器与键盘优先交互，在 1440×900 视口内同时承载多进程会话、工具 diff 审查、队列调度与子 agent 监控而不产生视觉疲劳。

---

## 1. 色彩系统（Semantic Tokens）

色彩命名严格对齐 `gpui-kit 0.7` 的 `ThemeColors` 语义 token，并扩展 coding agent 专用的 Diff、Thinking 与子进程状态 token。亮色模式采用暖调工业纸面底色以降低长时间阅读的眩光；暗色模式采用高对比深石墨底色，确保语法高亮与 diff 块层次清晰。

### 1.1 基础与表面语义 Token

| Token 名称（gpui-kit / 扩展） | CSS 变量映射 | Light（亮色 Hex） | Dark（暗色 Hex） | 用途与对比度约束 |
| --- | --- | --- | --- | --- |
| `background` | `--bg-background` | `#F7F6F2` | `#0F1115` | 主工作区背景、对话流画布底层 |
| `panel_background` | `--bg-panel` | `#EFECE6` | `#15181E` | 左侧会话栏、右侧 Subagent/Todo 面板、顶栏、底栏 |
| `elevated_surface` | `--bg-elevated` | `#FFFFFF` | `#1C2028` | 工具调用卡片头、模态对话框、Slash 补全浮层、Toast |
| `code_surface` | `--bg-code` | `#F2F0EC` | `#0B0D10` | 代码块、Bash 终端输出、Diff 视图底层 |
| `foreground` | `--fg-default` | `#16181D` | `#E6E8EE` | 正文、代码主文本、标题（对比度 $\ge 11.5:1$） |
| `muted` | `--bg-muted` | `#E4E0D8` | `#232832` | 次级按钮背景、代码块顶栏、未激活 Tab 悬停态、标签底色 |
| `muted_foreground` | `--fg-muted` | `#5C606B` | `#8E95A5` | 时间戳、路径前缀、耗时、Token 计数（对比度 $\ge 4.8:1$） |
| `subtle_foreground` | `--fg-subtle` | `#858A96` | `#636A78` | 占位符（Placeholder）、快捷键提示文本、禁用态文字 |
| `border` | `--border-default` | `#DCD8CE` | `#262B36` | 面板 1px 结构分割线、卡片边框、输入框默认边框 |
| `border_focused` | `--border-focus` | `#1E5EFF` | `#4C82FF` | 输入框聚焦、键盘导航焦点环、当前激活模态框边线 |

### 1.2 交互与状态语义 Token

| Token 名称 | CSS 变量映射 | Light（亮色 Hex） | Dark（暗色 Hex） | 用途与场景 |
| --- | --- | --- | --- | --- |
| `primary` | `--color-primary` | `#1849D6` | `#3B74FF` | 主操作按钮（发送、确认审批、重连）、激活项指示条 |
| `primary_foreground` | `--fg-on-primary` | `#FFFFFF` | `#FFFFFF` | `primary` 按钮上的文字与图标 |
| `accent` | `--bg-accent` | `#E6EEFF` | `#19284D` | 会话列表当前选中项背景、Ask 选项选中态底色 |
| `accent_foreground` | `--fg-accent` | `#123BB5` | `#8AB0FF` | 选中项高亮文字、`Recommended` 徽章文字、Steer 芯片主色 |
| `success` | `--color-success` | `#137347` | `#2EA063` | 进程已连接（Settled）、工具执行成功、`+` 新增行数统计 |
| `warning` | `--color-warning` | `#B25E09` | `#E5922A` | 思考档位（High/xHigh）、审批等待、上下文占用 $>80\%$ 预警 |
| `danger` | `--color-danger` | `#C82829` | `#F05252` | 进程已断开（`Error::Closed`）、Abort 按钮、拒绝执行、`-` 删除统计 |

### 1.3 Coding Agent 专用扩展 Token

| Token 名称 | CSS 变量映射 | Light（亮色 Hex） | Dark（暗色 Hex） | 用途与场景 |
| --- | --- | --- | --- | --- |
| `diff_added_bg` | `--diff-add-bg` | `#E6F6EC` | `#122B1E` | `edit` 工具卡片新增代码行背景 |
| `diff_added_gutter` | `--diff-add-gutter` | `#C7ECD5` | `#193D2A` | 新增行号栏背景与行内字符级高亮 |
| `diff_added_fg` | `--diff-add-fg` | `#0F5936` | `#67D697` | 新增行符号 `+` 与文本色 |
| `diff_removed_bg` | `--diff-del-bg` | `#FCECEB` | `#301619` | `edit` 工具卡片删除代码行背景 |
| `diff_removed_gutter` | `--diff-del-gutter` | `#F7D1CF` | `#471E22` | 删除行号栏背景与行内字符级高亮 |
| `diff_removed_fg` | `--diff-del-fg` | `#A31D1E` | `#F78686` | 删除行符号 `-` 与文本色 |
| `thinking_bg` | `--thinking-bg` | `#F1EFEA` | `#14171D` | `thinking_delta` 推理折叠块背景 |
| `thinking_border` | `--thinking-border` | `#D5D0C5` | `#282E3A` | 推理块 1px 结构边框 |
| `steer_chip_bg` | `--chip-steer-bg` | `#FFF3E0` | `#2D2112` | 队列中 `Steer`（立即插队转向）芯片底色 |
| `steer_chip_fg` | `--chip-steer-fg` | `#9E4F00` | `#F5A646` | 队列中 `Steer` 芯片文字与标签色 |

---

## 2. 字体与排版规范（Typography）

桌面工具类 UI 禁用流体字号（Fluid Clamp），全部使用固定像素阶梯与 `tabular-nums` 等宽数字，保证 token 消耗、费用、耗时及行号在高频刷新时绝不发生横向抖动。

### 2.1 字体栈（中英混排与 macOS 回退）

- **UI 界面与正文（Sans）**：  
  `"Inter", -apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif`
- **代码、路径、终端、Diff、状态栏指标（Mono）**：  
  `"JetBrains Mono", "SF Mono", "Menlo", "Monaco", "PingFang SC", monospace`
- **数字排版属性**：全局数据区、状态栏、耗时、行号强制启用 `font-variant-numeric: tabular-nums`。

### 2.2 字号与行高阶梯

| 级别 | Token | 字号 (px) | 行高 (px / 倍率) | 字重 | 典型应用场景 |
| --- | --- | --- | --- | --- | --- |
| `text-xs` | `11px` | `11px` | `16px` (`1.45`) | 500 | 状态栏指标、快捷键徽章、PID 标识、时间戳、队列类型标签 |
| `text-sm` | `12px` | `12px` | `18px` (`1.50`) | 400 / 500 | 会话列表项、Tab 标题、工具卡片头部、Diff 代码行、Bash 输出、Subagent 列表 |
| `text-base` | `13.5px` | `13.5px` | `21px` (`1.55`) | 400 | 对话流用户消息、助手 Markdown 正文、输入框文本、Ask 对话框题目说明 |
| `text-md` | `15px` | `15px` | `22px` (`1.46`) | 600 | Markdown 二级标题 (`##`)、模态框主标题、空状态小标题 |
| `text-lg` | `18px` | `18px` | `26px` (`1.44`) | 600 | Markdown 一级标题 (`#`)、首次启动异常页主标题、项目欢迎头 |

- **正文行宽（Measure）**：助手 Markdown 文本容器最大阅读行宽限制为 `74ch`（约 `680px`），而工具调用卡片（Diff / Bash）允许撑满对话流内容区最大宽度（`920px`）以减少代码折行。

---

## 3. 布局、间距与尺寸规范（Layout & Spacing）

### 3.1 间距与圆角阶梯

- **间距阶梯（4px 基准带 2px 微调）**：`2px`（内联徽章间距）、`4px`（紧凑按钮组）、`6px`（列表项内边距）、`8px`（工具卡片内部间距）、`12px`（面板内边距、消息块间距）、`16px`（对话流水平内边距）、`24px`（大区块留白）。
- **圆角阶梯（克制硬朗）**：
  - `radius-xs`: `4px`（快捷键 Kbd、状态徽章、行内 `code`、复选框）
  - `radius-sm`: `6px`（按钮、队列 Chip、下拉菜单项、会话列表选中块）
  - `radius-md`: `8px`（工具调用卡片、代码块、底部输入框外框）
  - `radius-lg`: `10px`（Ask / Approval 模态对话框、Slash 补全浮层）
- **Elevation（层级表达）**：严禁在同一元素混用宽大软阴影与粗边框。主界面面板全部使用 `1px solid var(--border-default)` 零阴影分割；仅浮层（Modal、Slash 补全、Toast）使用 `1px` 边框配合收敛阴影 `0 10px 28px -4px rgba(0, 0, 0, 0.18)`。

### 3.2 窗口与区域尺寸划分

```text
+-----------------------------------------------------------------------------------+
| [macOS Traffic Lights] | Tab 1 (omp-desktop) | Tab 2 (!已断开) | [+]   [Sidebar Toggle] | <- 36px 顶栏/Tab栏
+------------------+---------------------------------------------+------------------+
| 左侧会话栏        | 中央主工作区 (Chat Stream + Composer)         | 右侧监控栏 (可选)  |
| 256px (可拖拽    | flex-1 (最小 620px)                          | 280px (可折叠)   |
| 220px - 340px)   | - 顶部会话上下文条 (32px)                     | - Todo 进度阶段   |
| - 搜索框 (⌘P)    | - 虚拟滚动对话流 (flex-1, 内容限宽 920px)     | - Subagents 实时 |
| - 按 cwd 分组    | - 队列 Chip 栏 (按需展开)                     |   状态/Steer/Kill|
| - mtime 倒序     | - 多行输入框 Composer (88px - 260px)          |                  |
+------------------+---------------------------------------------+------------------+
| ● rpc-ui v2 (PID 48291) | claude-opus-5.5 | 思考: High | Ctx: 42% | $0.184 | 2 Sub | <- 26px 底部状态栏
+-----------------------------------------------------------------------------------+
```

- **窗口最小尺寸**：`1024 × 640 px`；推荐默认工作尺寸：`1440 × 900 px`。
- **顶栏多进程 Tab 区**：高度 `36px`（为 macOS 红黄绿窗口按钮预留左侧 `76px` 内边距）。
- **左侧会话栏**：默认宽度 `256px`，支持 `220px–340px` 拖拽调整，`⌘B` 一键折叠。
- **右侧辅助面板（Todo & Subagents）**：默认宽度 `280px`，支持 `⌘J` 或点击底部状态栏徽章展开/收起；当窗口宽度 $<1180\text{px}$ 时自动收起为浮层抽屉。
- **底部状态栏**：固定高度 `26px`，全等宽数字展示会话级实时遥测。

---

## 4. 核心组件结构、状态与交互规范

### 4.1 顶栏多进程会话 Tab（Multi-Process Session Tabs）

每个打开的 Tab 独立持有一个 `omp --mode rpc-ui` 子进程。

- **结构**：`[状态圆点 7px] + [项目目录名/会话短标题] + [状态角标（流式中脉冲/待审批警告/断线标记）] + [关闭按钮 ×]`，右侧紧随 `+` 新建会话按钮。
- **状态矩阵**：
  - **Idle / Settled（默认）**：圆点为 `--color-success` 静态实心，文字 `--fg-muted`（非激活）或 `--fg-default`（激活 Tab，底部 `2px` `--color-primary` 指示条）。
  - **Streaming（流式输出中）**：圆点为 `--color-primary` 呼吸脉冲，Tab 右侧显示微型 `•••` 节拍指示。
  - **Waiting Approval / Ask（等待人工介入）**：圆点变为 `--color-warning` 琥珀色，显示 `待确认` 胶囊徽章，提醒用户在其他 Tab 时切回处理。
  - **Disconnected（`Error::Closed` 进程异常退出）**：圆点变为 `--color-danger` 实心红点，标题后缀 `[已断开]`，激活时主区顶部弹出非模态重连横幅（含 `立即重连 --resume` 按钮）。
  - **Hover**：显示关闭按钮 `×`（点击触发 `Client::close()` 回收进程组）；若当前有后台 bash 任务，hover 提示“关闭将终止 omp 进程（独立后台进程组除外）”。

### 4.2 侧栏会话列表（Session Explorer：`~/.omp/agent/sessions`）

- **结构**：
  - 顶部：搜索过滤框（支持按标题、cwd 路径模糊筛选）+ `新建会话` 图标按钮。
  - 分组头：按 `<encoded-cwd>` 解码后的工作区路径分组（如 `~/Project/omp-desktop`），显示折叠箭头与会话数量。
  - 列表项（高 `52px`）：第一行显示 `title`（单行截断）+ 相对 `mtime`（如 `2m`、`昨天`）；第二行显示会话短 ID（如 `#8f3a`）与当前打开状态标记（`已打开` 徽章）。
- **状态矩阵**：
  - **Default**：透明底，文字 `--fg-default`。
  - **Hover**：底色 `--bg-muted`，右侧浮现 `Fork 分支` 与 `在 Finder 中显示 JSONL` 快捷图标。
  - **Selected（当前 Tab 绑定）**：底色 `--bg-accent`，`1px solid var(--border-focus)` 描边，标题字重 `600`。
  - **Parse Warning（JSONL 头 4KiB 损坏或版本不兼容）**：按技术方案 §3.5 静默跳过不渲染，底部小字可通过 hover 查看“已跳过 N 个不兼容会话文件”。

### 4.3 对话流（Conversation Stream & Incremental Delta）

采用“左对齐全宽工程日志流”而非气泡聊天样式，最大化代码与 Diff 可读性。

- **消息单元结构**：
  1. **用户消息（User Message）**：浅底色色块（`--bg-panel`），`1px solid var(--border-default)` 边框，右上角展示发送时间与 `Fork 此节点` / `编辑重发` 按钮；若附带粘贴图片则显示 `48×48px` 缩略图网格。
  2. **Thinking 折叠块（`thinking_delta`）**：
     - **Collapsed（完成后默认折叠）**：单行高 `28px`，`▸ 思考过程 · 4.2s (840 tokens)`，点击平滑展开。
     - **Streaming（正在思考）**：默认展开最高 `160px` 内部可滚动视图，`1px solid var(--thinking-border)` 边框，头部显示动态计时器 `2.8s...`。
  3. **助手 Markdown 与代码块（`text_delta`）**：
     - 标题、列表、表格、行内代码实时增量追加；未闭合的代码围栏自动补全渲染，避免流式闪烁。
     - 代码块顶栏（高 `28px`）：左侧显示语言标识 + 文件路径（如有），右侧提供 `复制代码` 与 `插入输入框` 按钮。
  4. **流式中状态（Streaming Footer）**：消息末尾紧随 `7×14px` 实心矩形光标（`--color-primary`），底部状态行显示 `正在生成回复 · 48 tok/s · [Esc 停止]`。

### 4.4 工具调用卡片（Tool Call Cards：`toolcall_start` → `toolcall_end`）

每种核心工具具备针对性结构，严禁做成千篇一律的 JSON 倾倒框：

- **通用卡片头部（高 `32px`）**：`[工具图标] + [工具名 (read / edit / bash)] + [主参数摘要（如文件路径或命令，等宽字体）] + [右侧状态徽章：运行中耗时 / Exit 0 / 失败 / 待审批] + [折叠切换]`。
- **`read` 卡片**：默认单行紧凑展示，如 `read  crates/omp-bridge/src/lib.rs:1-120`，右侧显示 `120 行 · 3.4 KB`，点击可展开查看读取片段。
- **`edit` 卡片（核心高频）**：
  - 头部显示目标文件路径、`+新增行数`（绿）与 `-删除行数`（红）统计、Hashline 快照 tag（如 `#0A73`）。
  - 内容区采用**双列行号统一 Diff 视图（Unified Diff with Dual Line Numbers）**：左侧旧行号、右侧新行号（等宽 `tabular-nums`，宽 `36px`），`+` 行使用 `--diff-add-bg`，行内变更 token 用 `--diff-add-gutter` 高亮；支持点击右上角切换 `Unified / Split`。
- **`bash` 卡片**：
  - 头部显示 `$ cargo test -p omp-store` 及工作目录 `cwd: crates/omp-store`。
  - 内容区为暗色/等宽终端视口（最高 `220px`，超出内部滚动，支持 ANSI 颜色映射），底部栏显示 `exit code: 0 · 1.42s`，若转入后台进程组则显示 `bg_1 (独立进程组运行中)` 标签。

### 4.5 队列 Chip 栏（Prompt Queue：`steer` / `follow-up`）

位于输入框正上方，当 Agent 正在执行且用户继续提交指令时出现：

- **结构**：水平流式或紧凑垂直栈，每条队列项为一个高 `28px` 的胶囊卡片：
  - `[类型徽章：STEER 转向 (琥珀色) / FOLLOW-UP 追问 (蓝灰色)]` + `[指令文本单行摘要]` + `[↑ 提升为 Steer]`（仅 Follow-up 显示）+ `[✎ 编辑回填]` + `[× 移除]`。
- **状态交互**：
  - 点击 `↑ 提升为 Steer`：立即将该条 Follow-up 转为 `steer` 模式，在下一个工具边界立即注入 Agent。
  - 点击 `× 移除`：从等待队列中撤回该消息。

### 4.6 多行输入框与补全浮层（Composer & Slash Autocomplete）

- **结构**：
  - **附件与上下文预览条（有内容时显示）**：展示剪贴板粘贴的截图缩略图（含尺寸标签与 `×` 删除）、当前引用的 `@file` 芯片。
  - **文本编辑区**：最小高度 `56px`（2 行），随内容自动增高至 `220px` 后内部滚动；原生支持 macOS 中文输入法（IME Marked Text 下划线预编辑区高亮，候选词窗精确定位在光标下方）。
  - **底部控制工具条（高 `32px`）**：
    - 左侧：`[+ 图片/文件]`、`[/ 命令]`、`[模型快切: claude-opus-5.5 ▾]`、`[思考: High ▾]`。
    - 右侧：字数/Token 估算 + 主操作按钮。
- **主操作按钮状态切换**：
  - **空闲（Settled）**：主按钮为 `[发送 ↵]`（`--color-primary`）。
  - **流式运行中（Busy）**：输入框文字非空时显示分体按钮 `[追加队列 ↵]` 与 `[立即转向 Steer ⌥↵]`；输入框为空或右侧独立常驻红色按钮 `[■ 停止生成 (Esc)]`（触发 `abort` 命令）。
- **Slash 命令补全浮层（`available_commands_update`）**：
  - 当行首输入 `/` 时在光标上方弹出（宽 `360px`，最高 `240px`），上下方向键选择，`Tab` 或 `Enter` 补全（如 `/compact`、`/model`、`/fork`、`/clear`），右侧显示命令说明与参数签名。

### 4.7 交互对话框（`extension_ui_request`）

根据技术方案 §3.4，所有服务端发起的 UI 请求以**锚定在输入框上方的内联决策卡（Inline Action Sheet）或居中聚焦模态框**呈现，并支持收到 `cancel (targetId)` 时立即自动销毁：

1. **`ask` 多问题结构化表单（核心展示组件）**：
   - **头部**：显示标题 `Agent 需要确认实现细节 (2 个问题)`、请求 ID 与超时倒计时（如有）。
   - **问题块（支持单选 Radio / 多选 Checkbox）**：
     - 每个选项包含 `选项标题` + `optionDetails 详细说明`。
     - 带有 `recommended: true` 的选项右侧渲染醒目的 `推荐 (Recommended)` 蓝色胶囊徽章，并在初次打开时作为默认焦点项。
     - 底部固定包含 `其他（自由输入）` 选项，选中后自动展开单行/多行文本框。
   - **底部操作栏**：显示 `[取消 (Esc)]` 与 `[提交回答 (⌘↵)]`，支持数字键 `1–9` 快速勾选。
2. **`select` 工具审批对话框（Tool Approval）**：
   - 顶部高亮待审批的高危操作（如 `bash: rm -rf target/debug` 或跨域网络请求），配合 `optionDetails` 展示三个带说明的标准选项：
     - `[1] 允许本次执行 (Allow once)` — 仅对当前 toolcall 放行
     - `[2] 本次会话始终允许 (Allow for session)` — 加入当前进程白名单
     - `[3] 拒绝并指示修改 (Deny & steer)` — 拦截执行并允许附加拒绝原因
3. **`confirm` / `input` / `editor`（含 `secret: true` 掩码输入）**：
   - `secret: true` 时使用密码掩码框 `••••••••••••`，右侧提供“按住显示明文”眼睛图标，底部明确标注安全提示：`密钥仅通过本机 stdio 管道直接传给 omp 进程，omp-desktop 不落盘存储`。

### 4.8 底部状态栏、右侧 Todo 与子 Agent 监控面板（Status Bar & Subagents）

- **底部状态栏（26px）各字段（全量映射 `get_state`）**：
  1. **连接与协议**：`● 已连接 (v2 · delta)` 或 `▲ 已断开`（点击打开进程诊断日志）。
  2. **模型与思考档位**：`auto/claude-opus-5.5` · `Thinking: High`（点击直接弹出切换菜单）。
  3. **上下文用量（Context Window）**：微型进度条 + `42% (84k / 200k)` + `[Compact 压缩]` 快捷按钮（超过 80% 变琥珀色，超过 95% 变红色）。
  4. **累计费用（Cost）**：`$0.184`（等宽数字，hover 显示输入/输出/缓存命中明细）。
  5. **Todo 汇总按钮**：`☑ Todo 2/4`（点击展开右侧面板 Todo 区）。
  6. **子 Agent 汇总按钮**：`⑂ Subagents: 2 运行中`（点击展开右侧子 Agent 面板）。
- **右侧子 Agent 面板（Subagent Monitor）**：
  - 展示并发子任务卡片（如 `scout · 扫描 omp-rpc 事件定义`、`task · 实现增量重建状态机`）。
  - 每张卡片包含：`[Agent 名称与类型徽章]` + `[实时阶段状态文本 / 耗时]` + `[最后一行工具活动预览]` + 两个直接控制按钮：`[Steer 干预]`（展开微型输入框发送指令）与 `[× 取消 (Kill)]`。

### 4.9 Toast 与状态通知（`notify` / `setStatus`）

- 位于窗口右上角（距离顶栏下沿 `12px`，右沿 `16px`），宽度 `320px`。
  - 区分 `info`、`warning`、`error` 三种 `1px` 语义色边框与状态图标，最多同时堆叠 3 条，默认 `4s` 自动消散（`error` 级别需手动关闭或提供恢复动作按钮）。

---

## 5. 特殊全局状态页设计（Empty / Disconnected / Onboarding Error）

### 5.1 空状态（New Session Empty State）

当新建 Tab 尚未发送任何消息时，中央工作区不留白，展示面向工程师的高效启动台：
- **项目上下文头**：当前工作目录完整路径（如 `/Users/yangchengxxyy/Project/omp-desktop`）、Git 当前分支（`feat/gpui-shell`）与未提交变更数。
- **高频快捷动作卡（2×2 紧凑网格，非装饰性）**：
  1. `审查当前 Git 工作区变更`（自动填入 `/diff` 或审查指令）
  2. `恢复最近一次中断的任务`（读取最近会话最后一条未完成 Todo）
  3. `切换模型与思考档位`（当前：`claude-opus-5.5 · High`）
  4. `调用架构 탐색 Scout 子 Agent`
- **快捷键速查条**：`⌘N` 新建 Tab · `⌘P` 搜索历史会话 · `⌘K` 命令面板 · `⌥↵` 立即 Steer。

### 5.2 进程断开状态（Process Disconnected：`Error::Closed`）

当 `omp --mode rpc-ui` 子进程因崩溃、OOM 或外部 `kill` 意外退出时：
- **保留全部已渲染历史**：绝不清空当前对话流或已收到的一半流式消息（半截消息末尾标记 `[流式中断]`）。
- **顶部常驻断线横幅（Banner）**：红底描边，明确列出退出原因（如 `omp 子进程异常退出 (PID 48291, signal: SIGTERM / Error::Closed)`）、关联的会话文件路径 `~/.omp/agent/sessions/.../2026-10-29_8f3a.jsonl`，右侧提供主按钮 `[立即重连 (--resume)]` 与次按钮 `[查看 stderr 诊断日志]`。
- **底部输入框锁定态**：输入框置为 `disabled` 半透明态，占位符文案改为 `会话进程已断开，请点击上方“立即重连”恢复会话后再发送消息`，主按钮变为 `[重连会话 ⌘R]`。

### 5.3 首次启动与环境异常页（Onboarding & Version/Protocol Error）

根据技术方案 §3.6，客户端启动执行 `omp --version` 与 `ready.supportedProtocolVersions` 校验。当检测不通过时展示全屏结构化诊断向导，包含三类真实错误诊断卡：

1. **未检测到 `omp` 可执行文件（Binary Not Found）**：
   - 说明已在 `/opt/homebrew/bin/omp`、`~/.cargo/bin/omp` 及 `$PATH` 中搜索无果。
   - 提供一键复制安装命令框与 `[手动指定 omp 路径...]` 文件选择器。
2. **版本过低（Version Mismatch）**：
   - 对比表展示：`当前已安装版本: v17.4.0`（红） vs `最低要求版本: v18.13.3`（绿）。
   - 提供升级命令 `omp update` 或 `brew upgrade omp` 一键复制按钮。
3. **协议不支持 v2（Protocol v2 Unsupported）**：
   - 展示握手返回的原始 JSON 摘要：`{"ready": {"supportedProtocolVersions": [1]}}`，明确指出客户端依赖 Protocol v2 的自动分页与 `messageUpdates: "delta"` 增量流能力。
- 底部操作栏：`[重新检测环境 (⌘R)]` + `[配置自定义二进制路径]`。

---

## 6. 键盘交互与全局快捷键（Keyboard First）

| 快捷键 (macOS) | 作用域 | 触发动作 |
| --- | --- | --- |
| `Enter` / `Shift + Enter` | 输入框 | 发送消息（忙碌时加入 Follow-up 队列） / 换行 |
| `Opt + Enter` (`⌥↵`) | 输入框（忙碌时） | 立即作为 `Steer` 消息发送，打断并在下一工具边界生效 |
| `Esc` | 全局 / 对话流 | 若有弹窗则关闭弹窗；若正在流式输出则立即发送 `abort` |
| `⌘ + N` / `⌘ + W` | 全局窗口 | 新建会话 Tab（拉起新 `omp` 进程） / 关闭当前 Tab 并回收进程组 |
| `⌘ + 1..9` | 全局窗口 | 切换到第 1..9 个打开的会话 Tab |
| `⌘ + P` | 全局窗口 | 聚焦左侧会话列表搜索框，快速切换历史会话 |
| `⌘ + B` / `⌘ + J` | 全局窗口 | 折叠/展开左侧会话栏 / 折叠/展开右侧 Todo & Subagent 面板 |
| `⌘ + M` / `⌘ + T` | 输入框 | 快速打开模型切换器 / 循环切换思考档位（Low → Medium → High → xHigh） |
| `1..9` / `⌘ + Enter` | Ask / 审批弹窗 | 快速选中对应序号选项 / 提交当前表单回答 |

---

## 7. 动效规范（Minimal Purposeful Motion）

严格遵循 Operate 模式准则：**仅在传达状态切换与消除布局跳动时使用动效，禁止任何页面入场编排动画**。

| 动效场景 | 持续时长 | 缓动函数 (Easing) | 行为说明 |
| --- | --- | --- | --- |
| **流式打字机光标闪烁** | `900ms` 循环 | `steps(2, start)` | 仅在 `text_delta` 停顿超过 `120ms` 时闪烁；高频输出期间保持常亮实心块以降低视觉闪烁感 |
| **Thinking / 工具卡片折叠展开** | `140ms` | `cubic-bezier(0.16, 1, 0.3, 1)` | 高度与透明度同步过渡；展开时保持滚动锚点稳定，不顶走上方内容 |
| **侧栏 / 右侧监控面板显隐** | `160ms` | `cubic-bezier(0.22, 1, 0.36, 1)` | 宽度过渡；主对话区使用弹性容器平滑重算测量宽度 |
| **Ask / Approval 弹窗唤起** | `120ms` | `ease-out` | `opacity: 0 → 1` 配合 `translateY(4px → 0)`，立即获得键盘焦点 |

---

## 8. GPUI + gpui-kit 0.7 实现映射表

客户端不写 CSS，全部通过 GPUI 的 Tailwind 风格链式方法（`.flex().items_center().bg(cx.theme().background)...`）构建。各核心 UI 模块与 `gpui-kit 0.7` 现成组件及自绘边界映射如下：

| 界面模块 | 复用 `gpui-kit 0.7` 现成组件 | 需要自绘 / 深度定制的 GPUI 视图 (`Render` 实现) | 关键状态与性能约束 |
| --- | --- | --- | --- |
| **主窗口骨架与三栏布局** | `DockArea` / `ResizablePanelGroup`、`TitleBar` | 顶栏内嵌多进程状态 Tab 条（`SessionTabBar`） | 左侧栏与右侧栏拖拽宽度持久化到本地配置 |
| **多进程会话 Tab 栏** | `Tabs`、`Tab`、`Badge`、`IconButton` | 自绘 Tab 内嵌的 4 态进程指示灯（Idle / Streaming / Approval / Disconnected） | 关闭 Tab 时异步放到 `background_executor` 调 `Client::close()`，防止阻塞 UI 线程 |
| **侧栏历史会话列表** | `Input`（搜索）、`VirtualList`（长列表）、`Collapsible`（按 cwd 分组） | 自绘 `SessionListItem`（双行布局 + Hover 快捷操作按钮） | 结合 `notify` 文件系统监听增量刷新，保持滚动位置不重置 |
| **对话流列表（核心）** | `VirtualList` 或 GPUI 原生 `uniform_list` / `list` | **必须深度定制动态高度 `list`（`ConversationListView`）** | `message_update (delta)` 每秒触发数十次，需固定未完成块的外框或仅对最后一个活跃 item 做增量重排，严禁全量重建列表 |
| **助手 Markdown 与代码高亮** | `Markdown`（内置段落/列表/表格渲染）、Tree-sitter `CodeBlock` | 定制未闭合围栏流式容错包装器 + 代码块顶栏 `Copy / Insert` 工具条 | 流式增量期间对已完成段落缓存 Layout，只重算尾部活跃段落 |
| **Thinking 思考折叠块** | `Collapsible`、`ScrollView` | 自绘带实时秒表计时与左侧引导线的 `ThinkingBlockView` | 收到 `message_end` 后若用户未手动触碰则自动收起 |
| **`edit` 工具 Diff 卡片** | Tree-sitter 语法高亮 Token 渲染器 | **自绘 `DiffCardView`（双列行号 + 行级/词级背景着色 + Unified/Split 切换）** | 使用等宽行高与 `uniform_list` 虚拟化超长文件 Diff（$>500$ 行时不卡顿） |
| **`bash` 终端输出卡片** | `ScrollView`、`Badge` | **自绘轻量 ANSI 转义序列富文本渲染器（`AnsiTerminalView`）** | 限制单卡最大保留行数（尾部环形缓冲 + `artifact://` 链接提示），防止巨型日志撑爆内存 |
| **队列 Chip 栏** | `Tag` / `Badge`、`Button`、`Tooltip` | 自绘 `QueueStripView`（Steer / Follow-up 列表与提升/删除动作绑定） | 直接订阅 `omp-store` 中的 `QueueState` Entity |
| **多行输入框与 Slash 补全** | `TextInput` / `Input`（含 IME 修复）、`Popover`、`DropdownMenu` | **自绘组合式 `ComposerView`（图片剪贴板粘贴预览条 + `/` 触发坐标跟随补全列表 + 忙碌态分体发送按钮）** | 严格处理 macOS 中文输入法 `marked_text` 期间回车上屏与发送键的冲突 |
| **`ask` / 审批 / 密钥对话框** | `Modal` / `Dialog`、`RadioGroup`、`Checkbox`、`Input`（`masked: true`） | **自绘多问题动态表单容器 `AskDialogView`（支持单选+多选+推荐徽章+自由输入混合编排及 `targetId` 远程取消）** | 收到服务端 `cancel` 事件时立即卸载对应 `targetId` 弹窗，返回顺序严格对齐请求 `questions[].id` |
| **底部状态栏与子 Agent 面板** | `StatusBar`、`Progress`、`Popover`、`Toast` | 自绘 `SubagentMonitorPanel`（订阅子 agent 实时进度并提供行内 `Steer` 与 `Kill` 交互） | 状态栏所有数字强制等宽，高频 token 跳字不引起右侧元素位移 |
