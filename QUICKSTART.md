# Quick Start Guide - 快速开始指南

## 1. 录制目标元素

```bash
cd D:\mouse-recorder
cargo run
```

### 操作步骤：
1. 按 `R` 键 - 标记区域起点
2. 移动鼠标到目标区域的对角
3. 再按 `R` 键 - 完成框选
4. 输入描述（如"登录按钮"）
5. 重复录制其他元素
6. 按 `S` 键 - 保存并导出

### 输出文件：
- `D:\airtest_records_YYYYMMDD_HHMMSS.csv`
- `D:\airtest_records_YYYYMMDD_HHMMSS.json`
- `D:\airtest_region_XXX_YYYYMMDD_HHMMSS.png`（每个区域的截图）

## 2. 在 Airtest 中使用

### 方法A：使用提供的自动化脚本

1. 编辑 `airtest_automation.py`：
```python
RECORDS_FILE = "D:/airtest_records_20260621_120000.json"  # 改成你的文件路径
```

2. 运行脚本：
```bash
python airtest_automation.py
```

### 方法B：手动集成到你的脚本

```python
import json
from airtest.core.api import *

# 加载录制数据
with open('D:/airtest_records_XXXXXX.json', 'r', encoding='utf-8') as f:
    records = json.load(f)

# 获取分辨率缩放因子
current_w, current_h = device().get_current_resolution()
original_w = records[0]['screen_resolution']['width']
original_h = records[0]['screen_resolution']['height']
scale_x = current_w / original_w
scale_y = current_h / original_h

# 点击录制的元素
for record in records:
    scaled_x = int(record['center_x'] * scale_x)
    scaled_y = int(record['center_y'] * scale_y)
    
    # 方式1: 图像识别
    try:
        touch(Template(record['image_path']))
    except:
        # 方式2: 坐标点击
        touch([scaled_x, scaled_y])
    
    sleep(1)
```

## 3. 常见问题

### Q: 如何修改保存路径？
A: 编辑 `src/main.rs` 中的路径：
```rust
let filename = format!("D:\\airtest_region_{:03}_{}.png", id, timestamp);
// 改成你想要的路径，例如：
let filename = format!("C:\\my_project\\images\\region_{:03}_{}.png", id, timestamp);
```

### Q: 跨机器使用时坐标不准确？
A: 确保启用了分辨率自动缩放：
```python
AUTO_SCALE = True  # 在 airtest_automation.py 中
```

### Q: 图像识别失败率高？
A: 降低识别阈值：
```python
RECOGNITION_THRESHOLD = 0.7  # 从 0.8 降低到 0.7
```

### Q: 需要录制单点坐标而不是区域？
A: 可以录制一个很小的区域（如 5x5 像素），中心点就是你要的坐标。

## 4. 高级用法

### 条件判断
```python
for record in records:
    if exists(Template(record['image_path'])):
        touch(Template(record['image_path']))
        print(f"找到并点击: {record['description']}")
    else:
        print(f"未找到: {record['description']}")
```

### 等待元素出现
```python
wait(Template(record['image_path']), timeout=10)
touch(Template(record['image_path']))
```

### 循环操作
```python
for _ in range(5):  # 重复5次
    for record in records:
        touch(Template(record['image_path']))
        sleep(1)
```

## 5. 技巧提示

✅ **录制时保持窗口最大化**：确保分辨率一致
✅ **录制清晰特征**：选择有明显特征的区域便于识别
✅ **避免动态内容**：不要录制会变化的文本或图像
✅ **合理命名**：给每个元素起清晰的描述名称
✅ **测试验证**：录制完成后先在相同分辨率测试

❌ **避免重叠区域**：不同元素的框选区域不要重叠
❌ **避免过小区域**：至少 20x20 像素以上
❌ **避免纯色区域**：纯色区域识别率低
