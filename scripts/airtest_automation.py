"""
Airtest 自动化脚本 - 使用 mouse-recorder 生成的坐标数据

使用说明：
1. 先用 mouse-recorder 录制目标元素
2. 将生成的 JSON 文件路径填入 RECORDS_FILE
3. 运行此脚本即可自动化操作
"""

import json
import os
from airtest.core.api import *

# ==================== 配置区域 ====================

# mouse-recorder 生成的 JSON 文件路径（改成你的导出文件）
RECORDS_FILE = "data/export_elements.json"

# mouse-recorder 的图片目录（DB 里存的是相对路径）
DATA_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "data"
)

# 是否启用分辨率自动缩放
AUTO_SCALE = True

# 图像识别置信度阈值（0-1，越高越严格）
RECOGNITION_THRESHOLD = 0.8

# 点击间隔（秒）
CLICK_INTERVAL = 1.0

# ==================== 工具函数 ====================

def load_records(json_path):
    """加载录制的坐标数据"""
    if not os.path.exists(json_path):
        raise FileNotFoundError(f"找不到文件: {json_path}")
    
    with open(json_path, 'r', encoding='utf-8') as f:
        records = json.load(f)
    
    print(f"✓ 已加载 {len(records)} 条记录")
    return records


def resolve_image_path(image_path):
    """DB 里可能是相对路径（如 scene/foo_-n.png）或旧 basename。"""
    if not image_path:
        return image_path
    if os.path.isabs(image_path) and os.path.exists(image_path):
        return image_path
    if not os.path.isabs(image_path):
        cand = os.path.join(DATA_DIR, image_path)
        if os.path.exists(cand):
            return cand
    cand = os.path.join(DATA_DIR, os.path.basename(image_path))
    if os.path.exists(cand):
        return cand
    return image_path


def get_current_resolution():
    """获取当前设备分辨率"""
    try:
        width, height = device().get_current_resolution()
        return width, height
    except:
        return 1920, 1080


def calculate_scale_factors(record, current_width, current_height):
    """计算分辨率缩放因子"""
    screen = record.get('screen_resolution') or {}
    original_width = screen.get('width', record.get('screen_width', 1920))
    original_height = screen.get('height', record.get('screen_height', 1080))
    
    scale_x = current_width / original_width
    scale_y = current_height / original_height
    
    return scale_x, scale_y


def scale_point(x, y, scale_x, scale_y):
    """缩放单个坐标点"""
    return int(x * scale_x), int(y * scale_y)


def scale_bbox(bbox, scale_x, scale_y):
    """缩放边界框"""
    return {
        'x': int(bbox['x'] * scale_x),
        'y': int(bbox['y'] * scale_y),
        'width': int(bbox['width'] * scale_x),
        'height': int(bbox['height'] * scale_y),
    }


def click_by_image(record, scale_x=1.0, scale_y=1.0, threshold=0.8):
    """通过图像识别点击"""
    image_path = resolve_image_path(record['image_path'])

    if not os.path.exists(image_path):
        raise FileNotFoundError(f"找不到图像文件: {image_path}")

    cx = record.get('screen_center_x', record.get('center_x'))
    cy = record.get('screen_center_y', record.get('center_y'))
    scaled_x, scaled_y = scale_point(cx, cy, scale_x, scale_y)
    
    try:
        template = Template(image_path, threshold=threshold, record_pos=[scaled_x, scaled_y])
        pos = exists(template)
        
        if pos:
            touch(template)
            return True, "图像识别成功"
        else:
            return False, "未找到匹配图像"
    except Exception as e:
        return False, f"图像识别失败: {str(e)}"


def click_by_coordinate(record, scale_x=1.0, scale_y=1.0):
    """通过坐标直接点击"""
    cx = record.get('screen_center_x', record.get('center_x'))
    cy = record.get('screen_center_y', record.get('center_y'))
    scaled_x, scaled_y = scale_point(cx, cy, scale_x, scale_y)
    
    try:
        touch([scaled_x, scaled_y])
        return True, f"坐标点击成功 ({scaled_x}, {scaled_y})"
    except Exception as e:
        return False, f"坐标点击失败: {str(e)}"


def smart_click(record, scale_x=1.0, scale_y=1.0, threshold=0.8, prefer_image=True):
    """
    智能点击：优先使用图像识别，失败则使用坐标
    
    Args:
        record: 记录对象
        scale_x: X轴缩放因子
        scale_y: Y轴缩放因子
        threshold: 图像识别阈值
        prefer_image: 是否优先使用图像识别
    
    Returns:
        (success, method, message)
    """
    desc = record.get('description', f"元素#{record['id']}")
    
    if prefer_image:
        success, msg = click_by_image(record, scale_x, scale_y, threshold)
        if success:
            return True, "图像识别", f"{desc}: {msg}"
        
        print(f"  ⚠ {desc} 图像识别失败，使用坐标备用方案")
        success, msg = click_by_coordinate(record, scale_x, scale_y)
        return success, "坐标", f"{desc}: {msg}"
    else:
        success, msg = click_by_coordinate(record, scale_x, scale_y)
        return success, "坐标", f"{desc}: {msg}"


# ==================== 主执行函数 ====================

def execute_automation(records_file, auto_scale=True, recognition_threshold=0.8, 
                       click_interval=1.0, prefer_image=True):
    """
    执行自动化流程
    
    Args:
        records_file: JSON记录文件路径
        auto_scale: 是否启用分辨率自动缩放
        recognition_threshold: 图像识别阈值
        click_interval: 点击间隔时间
        prefer_image: 是否优先使用图像识别
    """
    print("=" * 60)
    print("Airtest 自动化执行")
    print("=" * 60)
    
    records = load_records(records_file)
    
    current_width, current_height = get_current_resolution()
    print(f"✓ 当前分辨率: {current_width}x{current_height}")
    
    if records:
        first = records[0]
        screen = first.get('screen_resolution') or {}
        original_width = screen.get('width', first.get('screen_width', 1920))
        original_height = screen.get('height', first.get('screen_height', 1080))
        print(f"✓ 录制分辨率: {original_width}x{original_height}")
        
        if auto_scale:
            scale_x, scale_y = calculate_scale_factors(records[0], current_width, current_height)
            print(f"✓ 缩放因子: X={scale_x:.3f}, Y={scale_y:.3f}")
        else:
            scale_x, scale_y = 1.0, 1.0
            print("⚠ 分辨率缩放已禁用")
    else:
        print("✗ 没有可执行的记录")
        return
    
    print("\n开始执行自动化操作...\n")
    
    success_count = 0
    fail_count = 0
    
    for i, record in enumerate(records, 1):
        print(f"[{i}/{len(records)}] 执行: {record.get('description', f'元素#{record['id']}')}")
        
        success, method, msg = smart_click(
            record, 
            scale_x, 
            scale_y, 
            recognition_threshold, 
            prefer_image
        )
        
        if success:
            print(f"  ✓ {msg} (方法: {method})")
            success_count += 1
        else:
            print(f"  ✗ {msg}")
            fail_count += 1
        
        if i < len(records):
            sleep(click_interval)
    
    print("\n" + "=" * 60)
    print(f"执行完成: 成功 {success_count}/{len(records)}, 失败 {fail_count}/{len(records)}")
    print("=" * 60)


# ==================== 使用示例 ====================

if __name__ == "__main__":
    # 方式1: 使用默认配置
    execute_automation(
        RECORDS_FILE,
        auto_scale=AUTO_SCALE,
        recognition_threshold=RECOGNITION_THRESHOLD,
        click_interval=CLICK_INTERVAL,
        prefer_image=True  # 优先使用图像识别
    )
    
    # 方式2: 仅使用坐标点击（不使用图像识别）
    # execute_automation(
    #     RECORDS_FILE,
    #     auto_scale=AUTO_SCALE,
    #     click_interval=CLICK_INTERVAL,
    #     prefer_image=False
    # )
