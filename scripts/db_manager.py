"""
Database Manager for Multi-State Element Recorder

This utility provides advanced operations for managing the element database:
- Query and list elements
- Export in various formats
- Set primary states
- Batch operations
- Database maintenance
"""

import sqlite3
import json
import csv
import sys
import os
from pathlib import Path
from typing import List, Dict, Optional, Tuple

ROOT_DIR = Path(__file__).resolve().parents[1]
DB_PATH = str(ROOT_DIR / "data" / "mouse_recorder.db")
IMAGE_DIR = ROOT_DIR / "data"


def resolve_image_path(image_path: str) -> Path:
    """Resolve DB image_path (relative subpath, basename, or absolute) to a file."""
    p = Path(image_path)
    if p.is_absolute() and p.is_file():
        return p
    if not p.is_absolute():
        cand = IMAGE_DIR / p
        if cand.is_file():
            return cand
    cand = IMAGE_DIR / p.name
    if cand.is_file():
        return cand
    return p


def _origin_select(conn) -> str:
    """Select origin columns when the DB has been migrated, else zeroes."""
    cols = {r[1] for r in conn.execute("PRAGMA table_info(elements)").fetchall()}
    if {"origin_x", "origin_y"} <= cols:
        return "e.origin_x, e.origin_y"
    return "0 AS origin_x, 0 AS origin_y"


class ElementDatabase:
    def __init__(self, db_path: str = DB_PATH):
        self.db_path = db_path
        self.conn = None
    
    def __enter__(self):
        self.conn = sqlite3.connect(self.db_path)
        self.conn.row_factory = sqlite3.Row
        return self
    
    def __exit__(self, exc_type, exc_val, exc_tb):
        if self.conn:
            self.conn.close()
    
    def list_elements(self, name_filter: Optional[str] = None) -> List[Dict]:
        cursor = self.conn.cursor()
        
        if name_filter:
            query = "SELECT * FROM elements WHERE name LIKE ? ORDER BY id"
            cursor.execute(query, (f"%{name_filter}%",))
        else:
            query = "SELECT * FROM elements ORDER BY id"
            cursor.execute(query)
        
        elements = []
        for row in cursor.fetchall():
            element = dict(row)
            element['states'] = self.get_element_states(element['id'])
            elements.append(element)
        
        return elements
    
    def get_element_states(self, element_id: int) -> List[Dict]:
        cursor = self.conn.cursor()
        cursor.execute(
            "SELECT * FROM element_states WHERE element_id = ? ORDER BY id",
            (element_id,)
        )
        return [dict(row) for row in cursor.fetchall()]
    
    def set_primary_state(self, element_id: int, state_name: str) -> bool:
        cursor = self.conn.cursor()
        
        cursor.execute(
            "UPDATE element_states SET is_primary = 0 WHERE element_id = ?",
            (element_id,)
        )
        
        cursor.execute(
            "UPDATE element_states SET is_primary = 1 WHERE element_id = ? AND state_name = ?",
            (element_id, state_name)
        )
        
        self.conn.commit()
        return cursor.rowcount > 0
    
    def delete_state(self, element_id: int, state_name: str) -> bool:
        cursor = self.conn.cursor()
        
        cursor.execute(
            "SELECT COUNT(*) as count FROM element_states WHERE element_id = ?",
            (element_id,)
        )
        count = cursor.fetchone()[0]
        
        if count <= 1:
            print(f"Cannot delete the only state of element #{element_id}")
            return False
        
        cursor.execute(
            "SELECT is_primary FROM element_states WHERE element_id = ? AND state_name = ?",
            (element_id, state_name)
        )
        row = cursor.fetchone()
        
        if not row:
            print(f"State '{state_name}' not found for element #{element_id}")
            return False
        
        if row[0]:
            print(f"Cannot delete primary state. Set another state as primary first.")
            return False
        
        cursor.execute(
            "DELETE FROM element_states WHERE element_id = ? AND state_name = ?",
            (element_id, state_name)
        )
        
        self.conn.commit()
        return True
    
    def delete_element(self, element_id: int) -> bool:
        cursor = self.conn.cursor()
        
        cursor.execute("DELETE FROM element_states WHERE element_id = ?", (element_id,))
        cursor.execute("DELETE FROM elements WHERE id = ?", (element_id,))
        
        self.conn.commit()
        return cursor.rowcount > 0
    
    def export_to_clicker(self, output_path: str, element_ids: Optional[List[int]] = None):
        cursor = self.conn.cursor()
        origin_select = _origin_select(self.conn)

        if element_ids:
            placeholders = ','.join('?' * len(element_ids))
            query = f"""
                SELECT e.id, e.name, e.center_x, e.center_y, es.image_path,
                       e.screen_width, e.screen_height, {origin_select}
                FROM elements e
                JOIN element_states es ON e.id = es.element_id AND es.is_primary = 1
                WHERE e.id IN ({placeholders})
                ORDER BY e.id
            """
            cursor.execute(query, element_ids)
        else:
            query = f"""
                SELECT e.id, e.name, e.center_x, e.center_y, es.image_path,
                       e.screen_width, e.screen_height, {origin_select}
                FROM elements e
                JOIN element_states es ON e.id = es.element_id AND es.is_primary = 1
                ORDER BY e.id
            """
            cursor.execute(query)
        
        with open(output_path, 'w', encoding='utf-8-sig', newline='') as f:
            writer = csv.writer(f)
            writer.writerow(['id', 'x', 'y', 'description', 'template_path',
                             'original_width', 'original_height', 'origin_x', 'origin_y'])

            for row in cursor.fetchall():
                elem_id, name, center_x, center_y, image_path, sw, sh, ox, oy = row
                ox = ox or 0
                oy = oy or 0
                full = str(resolve_image_path(image_path))
                # x/y are virtual-screen pixels (image center + capture origin).
                writer.writerow([elem_id, center_x + ox, center_y + oy, name, full, sw, sh, ox, oy])
        
        return output_path
    
    def export_to_json(self, output_path: str, include_all_states: bool = False):
        elements = self.list_elements()
        
        if not include_all_states:
            for elem in elements:
                elem['states'] = [s for s in elem['states'] if s['is_primary']]

        for elem in elements:
            ox = elem.get('origin_x') or 0
            oy = elem.get('origin_y') or 0
            elem['screen_center_x'] = elem['center_x'] + ox
            elem['screen_center_y'] = elem['center_y'] + oy

        with open(output_path, 'w', encoding='utf-8') as f:
            json.dump(elements, f, indent=2, ensure_ascii=False)
        
        return output_path
    
    def export_state_matrix(self, output_path: str):
        elements = self.list_elements()
        
        matrix = {}
        for elem in elements:
            matrix[elem['name']] = {
                'element_id': elem['id'],
                'position': (elem['center_x'], elem['center_y']),
                'states': {s['state_name']: s['image_path'] for s in elem['states']}
            }
        
        with open(output_path, 'w', encoding='utf-8') as f:
            json.dump(matrix, f, indent=2, ensure_ascii=False)
        
        return output_path
    
    def get_statistics(self) -> Dict:
        cursor = self.conn.cursor()
        
        cursor.execute("SELECT COUNT(*) FROM elements")
        total_elements = cursor.fetchone()[0]
        
        cursor.execute("SELECT COUNT(*) FROM element_states")
        total_states = cursor.fetchone()[0]
        
        cursor.execute("""
            SELECT AVG(state_count) as avg_states
            FROM (
                SELECT element_id, COUNT(*) as state_count
                FROM element_states
                GROUP BY element_id
            )
        """)
        avg_states = cursor.fetchone()[0] or 0
        
        return {
            'total_elements': total_elements,
            'total_states': total_states,
            'avg_states_per_element': round(avg_states, 2)
        }
    
    def verify_integrity(self) -> Dict:
        cursor = self.conn.cursor()
        issues = []
        
        cursor.execute("SELECT id, name FROM elements")
        elements = cursor.fetchall()
        
        for elem_id, elem_name in elements:
            cursor.execute(
                "SELECT COUNT(*) FROM element_states WHERE element_id = ?",
                (elem_id,)
            )
            state_count = cursor.fetchone()[0]
            
            if state_count == 0:
                issues.append(f"Element #{elem_id} ({elem_name}) has no states")
            
            cursor.execute(
                "SELECT COUNT(*) FROM element_states WHERE element_id = ? AND is_primary = 1",
                (elem_id,)
            )
            primary_count = cursor.fetchone()[0]
            
            if primary_count == 0:
                issues.append(f"Element #{elem_id} ({elem_name}) has no primary state")
            elif primary_count > 1:
                issues.append(f"Element #{elem_id} ({elem_name}) has multiple primary states")
        
        cursor.execute("SELECT id, element_id, image_path FROM element_states")
        states = cursor.fetchall()
        
        missing_images = []
        for state_id, elem_id, image_path in states:
            if not resolve_image_path(image_path).is_file():
                missing_images.append((state_id, elem_id, image_path))
        
        if missing_images:
            issues.append(f"{len(missing_images)} image file(s) not found")
        
        return {
            'ok': len(issues) == 0,
            'issues': issues,
            'missing_images': missing_images
        }


def print_elements(elements: List[Dict]):
    for elem in elements:
        print(f"\n{'='*60}")
        print(f"Element #{elem['id']}: {elem['name']}")
        print(f"Position: ({elem['center_x']}, {elem['center_y']})")
        print(f"BBox: ({elem['bbox_x']}, {elem['bbox_y']}) {elem['bbox_width']}x{elem['bbox_height']}")
        print(f"Screen: {elem['screen_width']}x{elem['screen_height']}")
        print(f"Created: {elem['created_at']}")
        print(f"\nStates:")
        for state in elem['states']:
            primary_marker = " [PRIMARY]" if state['is_primary'] else ""
            print(f"  - {state['state_name']}{primary_marker}")
            print(f"    Path: {state['image_path']}")


def main():
    if len(sys.argv) < 2:
        print_help()
        return
    
    command = sys.argv[1]
    
    try:
        with ElementDatabase() as db:
            if command == "list":
                name_filter = sys.argv[2] if len(sys.argv) > 2 else None
                elements = db.list_elements(name_filter)
                print_elements(elements)
            
            elif command == "stats":
                stats = db.get_statistics()
                print("\n=== Database Statistics ===")
                print(f"Total Elements: {stats['total_elements']}")
                print(f"Total States: {stats['total_states']}")
                print(f"Avg States per Element: {stats['avg_states_per_element']}")
            
            elif command == "set-primary":
                if len(sys.argv) < 4:
                    print("Usage: python db_manager.py set-primary <element_id> <state_name>")
                    return
                
                element_id = int(sys.argv[2])
                state_name = sys.argv[3]
                
                if db.set_primary_state(element_id, state_name):
                    print(f"✓ Set '{state_name}' as primary state for element #{element_id}")
                else:
                    print(f"✗ Failed to set primary state")
            
            elif command == "delete-state":
                if len(sys.argv) < 4:
                    print("Usage: python db_manager.py delete-state <element_id> <state_name>")
                    return
                
                element_id = int(sys.argv[2])
                state_name = sys.argv[3]
                
                if db.delete_state(element_id, state_name):
                    print(f"✓ Deleted state '{state_name}' from element #{element_id}")
            
            elif command == "delete-element":
                if len(sys.argv) < 3:
                    print("Usage: python db_manager.py delete-element <element_id>")
                    return
                
                element_id = int(sys.argv[2])
                
                if db.delete_element(element_id):
                    print(f"✓ Deleted element #{element_id}")
            
            elif command == "export-clicker":
                output = sys.argv[2] if len(sys.argv) > 2 else str(ROOT_DIR / "data" / "export_clicker.csv")
                db.export_to_clicker(output)
                print(f"[OK] Exported to: {output}")
            
            elif command == "export-json":
                output = sys.argv[2] if len(sys.argv) > 2 else str(ROOT_DIR / "data" / "export_elements.json")
                all_states = "--all-states" in sys.argv
                db.export_to_json(output, all_states)
                print(f"[OK] Exported to: {output}")
            
            elif command == "export-matrix":
                output = sys.argv[2] if len(sys.argv) > 2 else str(ROOT_DIR / "data" / "state_matrix.json")
                db.export_state_matrix(output)
                print(f"[OK] Exported state matrix to: {output}")
            
            elif command == "verify":
                result = db.verify_integrity()
                print("\n=== Database Integrity Check ===")
                if result['ok']:
                    print("✓ All checks passed!")
                else:
                    print(f"✗ Found {len(result['issues'])} issue(s):\n")
                    for issue in result['issues']:
                        print(f"  - {issue}")
                    
                    if result['missing_images']:
                        print(f"\nMissing images:")
                        for state_id, elem_id, path in result['missing_images'][:10]:
                            print(f"  State #{state_id} (Element #{elem_id}): {path}")
                        if len(result['missing_images']) > 10:
                            print(f"  ... and {len(result['missing_images']) - 10} more")
            
            elif command == "backup":
                import shutil
                from datetime import datetime
                
                backup_dir = str(ROOT_DIR / "data" / "backups")
                os.makedirs(backup_dir, exist_ok=True)
                
                timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
                backup_path = f"{backup_dir}/mouse_recorder_{timestamp}.db"
                
                shutil.copy2(DB_PATH, backup_path)
                print(f"✓ Backup created: {backup_path}")
                
                backups = sorted([f for f in os.listdir(backup_dir) if f.endswith('.db')])
                if len(backups) > 5:
                    old_backups = backups[:-5]
                    for old in old_backups:
                        os.remove(os.path.join(backup_dir, old))
                        print(f"  Removed old backup: {old}")
            
            else:
                print(f"Unknown command: {command}")
                print_help()
    
    except FileNotFoundError:
        print(f"Database not found: {DB_PATH}")
        print("Run the mouse-recorder program first to create the database.")
    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()


def print_help():
    print("""
Database Manager for Multi-State Element Recorder

Usage: python db_manager.py <command> [arguments]

Commands:
  list [filter]              List all elements (optionally filter by name)
  stats                      Show database statistics
  verify                     Check database integrity and validate image files
  backup                     Create a timestamped backup of the database
  set-primary <id> <state>   Set a state as primary for an element
  delete-state <id> <state>  Delete a state from an element
  delete-element <id>        Delete an element and all its states
  export-clicker [path]      Export to mouse-clicker CSV format
  export-json [path]         Export to JSON format
  export-matrix [path]       Export state matrix (all states per element)

Examples:
  python db_manager.py list
  python db_manager.py list button
  python db_manager.py stats
  python db_manager.py verify
  python db_manager.py backup
  python db_manager.py set-primary 1 hover
  python db_manager.py delete-state 1 disabled
  python db_manager.py export-clicker D:/my_export.csv
  python db_manager.py export-json D:/elements.json --all-states
  python db_manager.py export-matrix D:/matrix.json
""")


if __name__ == "__main__":
    main()
