# Multi-State Element Recorder

## 系统概述

这是一个升级版的鼠标录制工具，专门为按键多状态管理设计，完美对接 `mouse-clicker` 程序。

### 核心特性

- ✅ **多状态管理**：一个按钮可以有多个视觉状态（正常、悬停、按下、禁用等）
- ✅ **SQLite 数据库**：持久化存储所有元素和状态
- ✅ **主状态标记**：每个元素有一个主状态用于导出
- ✅ **无缝集成**：一键导出到 mouse-clicker 格式

## 数据库结构

### elements 表
```sql
id              INTEGER PRIMARY KEY  -- 元素ID
name            TEXT                 -- 元素名称（如"登录按钮"）
center_x        INTEGER              -- 中心点X坐标
center_y        INTEGER              -- 中心点Y坐标
bbox_x          INTEGER              -- 边界框左上角X
bbox_y          INTEGER              -- 边界框左上角Y
bbox_width      INTEGER              -- 边界框宽度
bbox_height     INTEGER              -- 边界框高度
screen_width    INTEGER              -- 录制时屏幕宽度
screen_height   INTEGER              -- 录制时屏幕高度
created_at      TEXT                 -- 创建时间
```

### element_states 表
```sql
id              INTEGER PRIMARY KEY  -- 状态ID
element_id      INTEGER              -- 关联的元素ID
state_name      TEXT                 -- 状态名称（如"normal", "hover", "pressed"）
image_path      TEXT                 -- 图像文件路径
is_primary      BOOLEAN              -- 是否为主状态（用于导出）
created_at      TEXT                 -- 创建时间
```

## 使用流程

### 1. 录制新元素

```
1. 按 [N] 键
2. 移动鼠标到元素左上角，按 [N] 标记起点
3. 移动鼠标到元素右下角，再按 [N] 完成框选
4. 输入元素名称（如 "login_button"）
5. 输入状态名称（如 "normal"）
```

**结果**：
- 数据库中创建新元素记录
- 保存第一个状态图像（标记为主状态）
- 元素ID显示在界面上

### 2. 添加其他状态

```
1. 确保上一步的元素ID显示在界面
2. 按 [A] 键
3. 框选该元素的其他状态区域
4. 输入状态名称（如 "hover", "pressed", "disabled"）
```

**示例场景**：
- 录制"登录按钮"的 normal、hover、pressed 三种状态
- 录制"复选框"的 unchecked、checked、indeterminate 三种状态

### 3. 查看所有元素

按 [L] 键查看已录制的所有元素和状态：

```
=== All Elements and States ===

  Element #1: login_button (960, 540)
    * - normal      <- 主状态
      - hover
      - pressed

  Element #2: checkbox (300, 200)
    * - unchecked   <- 主状态
      - checked

Press any key to continue...
```

### 4. 导出到 mouse-clicker

```
1. 按 [E] 键
2. 输入文件名（不含扩展名，如 "game_ui"）
3. 自动生成 CSV 文件：D:\game_ui_clicker_YYYYMMDD_HHMMSS.csv
```

**CSV 格式**：
```csv
id,x,y,description,template_path
1,960,540,"login_button","D:\elements\login_button_20260621_120000.png"
2,300,200,"checkbox","D:\elements\checkbox_20260621_120030.png"
```

### 5. 在 mouse-clicker 中使用

1. 打开 `mouse-clicker` GUI
2. 点击 "Browse..." 加载生成的 CSV 文件
3. 点击 "Start" 开始自动化点击

**工作原理**：
- mouse-clicker 会先尝试图像识别（使用主状态图像）
- 如果识别失败，降级到坐标点击
- 支持不同分辨率的自动缩放

## 实际应用场景

### 场景1：游戏UI自动化

```
元素: start_game_button
  ├─ normal    (灰色按钮)      ← 主状态
  ├─ hover     (高亮按钮)
  └─ disabled  (半透明按钮)

元素: character_select
  ├─ warrior   (战士图标)      ← 主状态
  ├─ mage      (法师图标)
  └─ archer    (弓箭手图标)
```

### 场景2：表单自动化

```
元素: submit_button
  ├─ enabled   (蓝色可点击)    ← 主状态
  └─ disabled  (灰色不可点击)

元素: agree_checkbox
  ├─ unchecked (空框)          ← 主状态
  └─ checked   (打勾)
```

### 场景3：状态监控

```
元素: connection_status
  ├─ connected    (绿色图标)   ← 主状态
  ├─ connecting   (黄色图标)
  └─ disconnected (红色图标)
```

## 高级用法

### 手动切换主状态

```sql
-- 将某个状态设置为主状态
UPDATE element_states SET is_primary = 0 WHERE element_id = 1;
UPDATE element_states SET is_primary = 1 WHERE id = 5;
```

### 查询特定状态的图像路径

```sql
SELECT e.name, es.state_name, es.image_path
FROM elements e
JOIN element_states es ON e.id = es.element_id
WHERE e.name = 'login_button' AND es.state_name = 'hover';
```

### 批量导出所有状态

```python
import sqlite3
import json

conn = sqlite3.connect('D:/elements/mouse_recorder.db')
cursor = conn.cursor()

cursor.execute("""
    SELECT e.id, e.name, es.state_name, es.image_path
    FROM elements e
    JOIN element_states es ON e.id = es.element_id
    ORDER BY e.id, es.id
""")

elements = {}
for row in cursor.fetchall():
    elem_id, elem_name, state_name, image_path = row
    if elem_name not in elements:
        elements[elem_name] = {}
    elements[elem_name][state_name] = image_path

with open('D:/all_states.json', 'w', encoding='utf-8') as f:
    json.dump(elements, f, indent=2, ensure_ascii=False)

print(f"Exported {len(elements)} elements")
```

## 文件组织

```
D:\
├── elements\                      # 截图和数据库存储目录
│   ├── mouse_recorder.db          # SQLite 数据库
│   ├── login_button_20260621_120000.png
│   ├── login_button_hover_20260621_120010.png
│   ├── checkbox_unchecked_20260621_120030.png
│   └── checkbox_checked_20260621_120040.png
└── game_ui_clicker_20260621_120100.csv  # 导出的 CSV
```

## 最佳实践

### ✅ 推荐做法

1. **命名规范**：使用小写字母和下划线（如 `login_button` 而不是 `登录按钮`）
2. **状态命名**：使用标准状态名（normal, hover, pressed, disabled, active, inactive）
3. **框选精确**：确保每个状态的框选区域一致
4. **主状态选择**：将最常见的状态设为主状态
5. **定期备份**：备份 `elements\` 目录（包含数据库和截图）

### ❌ 避免做法

1. ❌ 不同元素使用相同名称
2. ❌ 框选区域包含动态内容（如文字滚动）
3. ❌ 录制半透明或渐变效果（识别率低）
4. ❌ 状态图像差异过小（如颜色仅略有不同）

## 故障排除

### Q: 导出的 CSV 在 mouse-clicker 中无法识别图像？
A: 检查：
- 图像文件路径是否正确（绝对路径）
- 图像文件是否存在
- 降低 mouse-clicker 的识别阈值（从 0.8 降到 0.7）

### Q: 如何重新录制某个元素的状态？
A: 
1. 删除旧状态：`DELETE FROM element_states WHERE element_id = X AND state_name = 'Y'`
2. 按 [A] 重新录制

### Q: 数据库损坏怎么办？
A: 
1. 备份当前数据库：`copy D:\elements\mouse_recorder.db D:\elements\mouse_recorder_backup.db`
2. 重新初始化：删除 `D:\elements\mouse_recorder.db`，程序会自动创建新数据库
3. 图像文件仍然保留在 `elements\` 目录

### Q: 如何迁移到新机器？
A: 复制以下文件：
- 整个 `D:\elements\` 目录（包含数据库和所有截图）
- 导出的 CSV 文件

## 版本历史

### v0.3.0 (当前版本)
- ✨ 多状态元素管理
- ✨ SQLite 数据库存储
- ✨ 一键导出到 mouse-clicker
- ✨ 主状态标记系统

### v0.2.0
- 基础区域录制
- Airtest 集成

### v0.1.0
- 单点坐标记录
