"""
Safe Export Wrapper for mouse-recorder

Validates DB integrity, backs up, then exports CSV compatible with mouse-clicker.
"""

import sqlite3
import sys
import os
from pathlib import Path
from datetime import datetime
import shutil

ROOT_DIR = Path(__file__).resolve().parents[1]
DB_PATH = str(ROOT_DIR / "data" / "mouse_recorder.db")
IMAGE_DIR = ROOT_DIR / "data"


def resolve_image_path(image_path: str) -> Path:
    p = Path(image_path)
    if p.is_file():
        return p
    cand = IMAGE_DIR / p.name
    if cand.is_file():
        return cand
    return p


def verify_before_export():
    if not os.path.exists(DB_PATH):
        print(f"[FAIL] Database not found: {DB_PATH}")
        return False

    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    cursor.execute("""
        SELECT e.id, e.name, COUNT(es.id) as state_count,
               SUM(CASE WHEN es.is_primary = 1 THEN 1 ELSE 0 END) as primary_count
        FROM elements e
        LEFT JOIN element_states es ON e.id = es.element_id
        GROUP BY e.id
    """)

    issues = []
    for elem_id, elem_name, state_count, primary_count in cursor.fetchall():
        if state_count == 0:
            issues.append(f"Element #{elem_id} ({elem_name}) has no states")
        if primary_count == 0:
            issues.append(f"Element #{elem_id} ({elem_name}) has no primary state")
        elif primary_count > 1:
            issues.append(f"Element #{elem_id} ({elem_name}) has {primary_count} primary states")

    cursor.execute("""
        SELECT es.id, es.element_id, e.name, es.image_path
        FROM element_states es
        JOIN elements e ON es.element_id = e.id
        WHERE es.is_primary = 1
    """)

    missing_images = []
    for _state_id, elem_id, elem_name, image_path in cursor.fetchall():
        if not resolve_image_path(image_path).is_file():
            missing_images.append(f"Element #{elem_id} ({elem_name}): {image_path}")

    conn.close()

    if issues or missing_images:
        print("[FAIL] Integrity check failed:\n")
        if issues:
            print("Database issues:")
            for issue in issues:
                print(f"  - {issue}")
        if missing_images:
            print("\nMissing image files:")
            for img in missing_images:
                print(f"  - {img}")
        return False

    print("[OK] Integrity check passed")
    return True


def create_backup():
    backup_dir = str(ROOT_DIR / "data" / "backups")
    os.makedirs(backup_dir, exist_ok=True)

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    backup_path = f"{backup_dir}/mouse_recorder_{timestamp}.db"

    shutil.copy2(DB_PATH, backup_path)
    print(f"[OK] Backup created: {backup_path}")

    backups = sorted([f for f in os.listdir(backup_dir) if f.endswith('.db')])
    if len(backups) > 5:
        for old in backups[:-5]:
            os.remove(os.path.join(backup_dir, old))


def export_validated(output_path):
    conn = sqlite3.connect(DB_PATH)
    cursor = conn.cursor()

    cursor.execute("""
        SELECT e.id, e.name, e.center_x, e.center_y, es.image_path,
               e.screen_width, e.screen_height
        FROM elements e
        JOIN element_states es ON e.id = es.element_id AND es.is_primary = 1
        ORDER BY e.id
    """)

    rows = cursor.fetchall()
    conn.close()

    with open(output_path, 'w', encoding='utf-8-sig', newline='') as f:
        f.write("id,x,y,description,template_path,original_width,original_height\n")
        for row in rows:
            elem_id, name, center_x, center_y, image_path, sw, sh = row
            full = str(resolve_image_path(image_path))
            escaped_name = name.replace('"', '""')
            escaped_path = full.replace('"', '""')
            f.write(f'{elem_id},{center_x},{center_y},"{escaped_name}","{escaped_path}",{sw},{sh}\n')

    print(f"[OK] Exported {len(rows)} element(s) to: {output_path}")
    return True


def main():
    if len(sys.argv) < 2:
        print("Usage: python scripts/safe_export.py <output_file.csv>")
        print("\nExample: python scripts/safe_export.py data/game_ui.csv")
        return

    output_path = sys.argv[1]

    print("=== Safe Export Process ===\n")

    print("Step 1: Verifying database integrity...")
    if not verify_before_export():
        print("\n[FAIL] Export aborted due to integrity issues")
        print("Run 'python scripts/db_manager.py verify' for details")
        sys.exit(1)

    print("\nStep 2: Creating backup...")
    create_backup()

    print("\nStep 3: Exporting to CSV...")
    if export_validated(output_path):
        print("\n[OK] Safe export completed!")
        print("\nNext steps:")
        print("  1. Open mouse-clicker")
        print(f"  2. Load: {output_path}")
        print("  3. Test with a few clicks first")
    else:
        print("\n[FAIL] Export failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
