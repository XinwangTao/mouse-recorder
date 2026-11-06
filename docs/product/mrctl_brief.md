# mrctl 产品 brief

日期：2026-11-06
状态：已确认并实现（0.7.0）

## 用户与问题

同一个 `mouse-recorder.exe` 既要保留 GUI 录制，也要让脚本/Agent 通过命令行完成：
元素管理、自动找按键、点击回放、成功判定、失败反馈和自我迭代。

## In Scope

- `mouse-recorder.exe --cli <command>` 子命令，复用现有 DB / 图片目录 / 元素表。
- 目标发现三路：Windows UIA 无障碍树（标准 Win32/WPF/UWP）、模板匹配（通用/游戏）、OCR 文本定位。
- 非交互录制：给定矩形/窗口/UIA 控件，直接生成元素和截图。
- 点击工作流：JSON 与 YAML 双格式；步骤含 action、target、timeout、retry、expect。
- 动作：click / double_click / right_click / type / key / wait / assert。
- 判定：模板分数、窗口标题/进程、OCR 文本存在、元素状态。
- 闭环反馈：每步截图、分数、矩形、耗时、错误；JSON 报告 + 退出码。
- 自适应阈值：按历史证据在 0.50–0.98 内调整；连续失败标记“需重录”。
- `--json` 输出，方便 Agent/脚本解析。

## Out of Scope（本阶段）

- 非 Windows 平台。
- 浏览器 DOM 级自动化（交给 Playwright）。
- 反作弊/内核级输入（只用标准 SendInput）。
- 常驻调度服务/多机分发（先不做 `serve`）。
- 自动改 GUI 代码本身；自迭代只改工作流阈值和证据，不改 Rust 源码。

## 可验收成功标准

1. `--cli uia --window "记事本" --json` 返回控件树和屏幕矩形。
2. `--cli record --name scene/btn --rect x,y,w,h` 无 GUI 生成 DB 行 + PNG。
3. `--cli run flow.yaml --json` 按步点击，逐步截图/打分，生成报告；失败退出码非 0。
4. 同一工作流 JSON/YAML 都能跑；报告能看出哪一步、什么分数、为什么失败。
5. 连续失败时可自动调阈值或标记重录，并写进报告。
6. 现有 GUI 功能都能在 `--cli help` 找到等价命令。

## 主路径（1 条）

CLI 解析 → 加载 JSON/YAML → 定位目标（UIA/模板/OCR）→ 换算屏幕坐标（origin/DPI）→ 执行动作 → 判定成功 → 写证据报告 → 退出码。

## 关键失败 / 人工兜底

- UIA 拿不到（游戏/自绘）：自动回退模板；模板也低分则 OCR；三路都失败标记“需人工录制”。
- 动作后判定失败：有界重试（默认 2 次，可配）；仍失败写截图 + 分数 + 候选矩形，退出码非 0。
- 目标窗口卡死/无响应：UIA 调用带超时，超时回退模板/报错，不无限挂起。
- 自适应阈值只在证据充分时调整；连续失败反向调整并标记重录，不静默改到很低。

## 关键约束

- Windows 10/11；单 exe；不新增外部服务依赖。
- 坐标统一走现有“图像坐标 + origin”模型，避免多屏/DPI 回归。
- 工作流文件版本化（schema_version），旧文件必须能读或明确报错。
- 证据目录 `runs/<timestamp>/`，不污染 `data/`。

## 借鉴调研（1 回合）

- **pywinauto**（Python，UIA/Win32 后端）：采纳“控件选择器”思路（name/automation_id/class/type），自研 Rust 实现避免 Python 依赖。
- **FlaUI**（.NET UIA）：采纳“条件组合 + 超时重试”；不采用 .NET 运行时。
- **SikuliX**（图像自动化）：采纳“模板分 + 重试 + 截图证据”；OCR 作为第三路。
- **AutoHotkey**：脚本能力强但非结构化工作流，不采用。
- **Playwright**：只覆盖浏览器，不在本阶段范围。
- 对架构的 3 条直接影响：① UIA 作为首选定位，模板/OCR 为回退；② 工作流是数据文件，不是编译进代码的流程；③ 每步必须产出可回看的证据，自适应只基于证据。

## 待确认（默认值，不阻塞图）

- 证据目录：`runs/<timestamp>/`；报告 `report.json` + 每步 PNG。
- 自适应阈值边界 0.50–0.98，连续 3 次同向才调整。
- YAML/JSON 同一 schema；YAML 仅解析层不同。
- OCR 优先用 Windows 自带 OCR（WinRT），装不了再退回 Tesseract。
