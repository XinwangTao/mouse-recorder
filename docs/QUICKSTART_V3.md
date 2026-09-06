# Quick Start - Multi-State System

## 完整工作流程

### 步骤 1: 录制元素和多个状态

```bash
cd D:\mouse-recorder
cargo run
```

#### 录制"登录按钮"示例

1. 按 `N` 键开始
2. 框选按钮的**正常状态**区域
3. 输入名称: `login_button`
4. 输入状态: `normal`
5. ✓ 元素创建成功，显示 ID: 1

6. 按 `A` 键添加状态
7. 框选按钮的**悬停状态**区域
8. 输入状态: `hover`
9. ✓ 状态添加成功

10. 按 `A` 键继续添加
11. 框选按钮的**按下状态**区域
12. 输入状态: `pressed`
13. ✓ 状态添加成功

#### 录制"复选框"示例

1. 按 `N` 键开始新元素
2. 框选复选框的**未选中状态**
3. 输入名称: `agree_checkbox`
4. 输入状态: `unchecked`
5. ✓ 元素创建成功，显示 ID: 2

6. 按 `A` 键添加状态
7. 框选复选框的**选中状态**
8. 输入状态: `checked`
9. ✓ 状态添加成功

### 步骤 2: 查看已录制内容

按 `L` 键查看：

```
=== All Elements and States ===

  Element #1: login_button (960, 540)
    * - normal      <- 主状态
      - hover
      - pressed

  Element #2: agree_checkbox (300, 200)
    * - unchecked   <- 主状态
      - checked

Press any key to continue...
```

### 步骤 3: 导出到 mouse-clicker

```
1. 按 [E] 键
2. 输入文件名: game_ui
3. ✓ 生成: D:\game_ui_clicker_20260621_120000.csv
```

CSV 内容：
```csv
id,x,y,description,template_path
1,960,540,"login_button","D:\elements\login_button_20260621_120000.png"
2,300,200,"agree_checkbox","D:\elements\agree_checkbox_20260621_120030.png"
```

### 步骤 4: 在 mouse-clicker 中使用

1. 打开 `D:\mouse-clicker\target\release\mouse-clicker.exe`
2. 点击 "Browse..." 选择生成的 CSV 文件
3. 设置延迟时间（如 1000 毫秒）
4. 点击 "Start (3s countdown)"
5. 程序会：
   - 先尝试图像识别（使用主状态图像）
   - 如果找不到，使用坐标点击
   - 按顺序点击所有元素

## 数据库管理

### 查看所有元素

```bash
python db_manager.py list
```

### 查看统计信息

```bash
python db_manager.py stats
```

输出：
```
=== Database Statistics ===
Total Elements: 5
Total States: 12
Avg States per Element: 2.4
```

### 切换主状态

假设你想将 `login_button` 的主状态从 `normal` 改为 `hover`：

```bash
python db_manager.py set-primary 1 hover
```

### 删除某个状态

```bash
python db_manager.py delete-state 1 pressed
```

### 删除整个元素

```bash
python db_manager.py delete-element 2
```

### 批量导出

导出所有元素（仅主状态）：
```bash
python db_manager.py export-clicker D:\export.csv
```

导出所有状态到 JSON：
```bash
python db_manager.py export-json D:\all_elements.json --all-states
```

导出状态矩阵：
```bash
python db_manager.py export-matrix D:\matrix.json
```

## 高级场景

### 场景1: 游戏自动化

**需求**：自动点击游戏中的多个按钮

```
录制元素:
1. start_button (开始游戏)
   - normal, hover, disabled
2. character_warrior (选择战士)
   - unselected, selected
3. character_mage (选择法师)
   - unselected, selected
4. confirm_button (确认)
   - normal, hover

导出 → mouse-clicker → 自动执行
```

### 场景2: 表单自动填写

**需求**：自动填写并提交表单

```
录制元素:
1. username_field (用户名输入框)
   - empty, focused
2. password_field (密码输入框)
   - empty, focused
3. remember_checkbox (记住我)
   - unchecked, checked
4. login_button (登录按钮)
   - enabled, disabled

配合 Airtest:
- 用 touch() 点击输入框
- 用 text() 输入内容
- 用 touch() 点击提交
```

### 场景3: 状态监控

**需求**：监控应用状态并自动响应

```python
import sqlite3
from airtest.core.api import *

conn = sqlite3.connect('D:/elements/mouse_recorder.db')
cursor = conn.cursor()

cursor.execute("""
    SELECT es.state_name, es.image_path
    FROM elements e
    JOIN element_states es ON e.id = es.element_id
    WHERE e.name = 'connection_status'
""")

states = {row[0]: row[1] for row in cursor.fetchall()}

while True:
    if exists(Template(states['disconnected'])):
        print("❌ 连接断开，尝试重连...")
        touch(Template(states['reconnect_button']))
    elif exists(Template(states['connected'])):
        print("✓ 连接正常")
    
    sleep(5)
```

## 常用命令速查

| 操作 | 按键 / 命令 |
|------|------------|
| 创建新元素 | `N` |
| 添加状态 | `A` |
| 取消框选 | `C` |
| 查看列表 | `L` |
| 导出CSV | `E` |
| 退出程序 | `Esc` |
| 查看数据库 | `python db_manager.py list` |
| 切换主状态 | `python db_manager.py set-primary <id> <state>` |
| 删除状态 | `python db_manager.py delete-state <id> <state>` |

## 文件位置

```
D:\
├── elements\                            # 截图和数据库目录
│   ├── mouse_recorder.db                # 数据库
│   ├── login_button_20260621_120000.png
│   └── agree_checkbox_20260621_120030.png
├── *_clicker_*.csv                      # 导出的CSV
└── mouse-clicker\                       # 点击器程序
    └── target\release\mouse-clicker.exe
```

## 故障排除

### 问题：图像识别失败率高

**解决方案**：
1. 降低识别阈值（mouse-clicker 中修改 threshold 参数）
2. 确保框选区域没有动态内容
3. 切换到更稳定的状态作为主状态

### 问题：跨机器坐标不准

**解决方案**：
1. 记录时使用常见分辨率（1920x1080）
2. 在 mouse-clicker 中启用分辨率缩放
3. 优先使用图像识别而非坐标

### 问题：数据库文件过大

**解决方案**：
```bash
# 清理旧的未使用元素
python db_manager.py list
python db_manager.py delete-element <unused_id>

# 压缩数据库
sqlite3 D:\elements\mouse_recorder.db "VACUUM;"
```

## 下一步

1. ✅ 录制完所有元素
2. ✅ 导出到 mouse-clicker
3. ✅ 测试自动化流程
4. 📝 记录元素名称和状态（建议维护一个文档）
5. 🔄 定期备份数据库和图像文件
