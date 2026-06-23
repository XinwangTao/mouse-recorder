#!/usr/bin/env python3
"""
Mouse Recorder - Unified Management Tool

All-in-one command for managing the multi-state element recorder system.
"""

import sys
import os
import subprocess
from pathlib import Path

TOOLS = {
    'verify': {
        'cmd': ['python', 'db_manager.py', 'verify'],
        'desc': 'Check database integrity and validate images'
    },
    'backup': {
        'cmd': ['python', 'db_manager.py', 'backup'],
        'desc': 'Create timestamped database backup'
    },
    'list': {
        'cmd': ['python', 'db_manager.py', 'list'],
        'desc': 'List all recorded elements and states'
    },
    'stats': {
        'cmd': ['python', 'db_manager.py', 'stats'],
        'desc': 'Show database statistics'
    },
    'export': {
        'cmd': ['python', 'safe_export.py'],
        'desc': 'Safe export to mouse-clicker CSV (with validation)',
        'args': '<output.csv>'
    },
    'record': {
        'cmd': ['cargo', 'run'],
        'desc': 'Start element recorder (Rust program)',
        'needs_compile': True
    },
    'build': {
        'cmd': ['cargo', 'build', '--release'],
        'desc': 'Compile Rust program'
    },
}

def print_banner():
    print("""
╔════════════════════════════════════════════════╗
║   Mouse Recorder - Management Tool v0.3       ║
║   Multi-State Element Recording System        ║
╚════════════════════════════════════════════════╝
""")

def print_help():
    print_banner()
    print("Usage: python manage.py <command> [args]\n")
    print("Commands:")
    print()
    
    for cmd, info in TOOLS.items():
        args_str = f" {info['args']}" if 'args' in info else ""
        needs_str = " [needs Rust]" if info.get('needs_compile') else ""
        print(f"  {cmd:<12} {info['desc']}{needs_str}")
        if args_str:
            print(f"               Usage: manage.py {cmd}{args_str}")
    
    print()
    print("Workflow Examples:")
    print()
    print("  # Quick health check")
    print("  python manage.py verify")
    print()
    print("  # Record new elements")
    print("  python manage.py record")
    print()
    print("  # Export for mouse-clicker")
    print("  python manage.py export D:/game_ui.csv")
    print()
    print("  # View all elements")
    print("  python manage.py list")
    print()
    print("Documentation:")
    print("  QUICKSTART_V3.md              - Quick start guide")
    print("  MULTI_STATE_GUIDE.md          - Complete system guide")
    print("  RUST_FIXES_REQUIRED.md        - Rust code fixes needed")
    print("  ADVERSARIAL_REVIEW_SUMMARY.md - Review report")
    print()

def run_command(cmd_name, extra_args=None):
    if cmd_name not in TOOLS:
        print(f"✗ Unknown command: {cmd_name}")
        print(f"Run 'python manage.py help' for available commands")
        return False
    
    tool = TOOLS[cmd_name]
    
    if tool.get('needs_compile'):
        if not Path('target/release/mouse-recorder.exe').exists() and \
           not Path('target/release/mouse-recorder').exists():
            print("⚠️  Warning: Rust program not compiled yet")
            print("   Run: python manage.py build")
            print()
            response = input("Try to build now? (y/n): ")
            if response.lower() == 'y':
                print("\nBuilding...")
                subprocess.run(['cargo', 'build', '--release'])
    
    cmd = tool['cmd'].copy()
    if extra_args:
        cmd.extend(extra_args)
    
    print(f"Running: {' '.join(cmd)}")
    print()
    
    try:
        result = subprocess.run(cmd, check=False)
        return result.returncode == 0
    except FileNotFoundError as e:
        print(f"✗ Error: {e}")
        if 'cargo' in cmd:
            print("   Rust toolchain not found. Install from: https://rustup.rs/")
        return False

def quick_status():
    print_banner()
    
    db_path = "D:/elements/mouse_recorder.db"
    elements_dir = "D:/elements"
    config_path = "config.toml"
    rust_binary = "target/release/mouse-recorder.exe"
    
    print("System Status Check:")
    print()
    
    checks = [
        ("Database", Path(db_path).exists()),
        ("Images directory", Path(elements_dir).exists()),
        ("Config file", Path(config_path).exists()),
        ("Rust binary", Path(rust_binary).exists()),
        ("Python tools", Path("db_manager.py").exists()),
        ("Safe export", Path("safe_export.py").exists()),
    ]
    
    for name, exists in checks:
        status = "✓" if exists else "✗"
        print(f"  {status} {name}")
    
    print()
    
    if Path(db_path).exists():
        try:
            import sqlite3
            conn = sqlite3.connect(db_path)
            cursor = conn.cursor()
            
            cursor.execute("SELECT COUNT(*) FROM elements")
            elem_count = cursor.fetchone()[0]
            
            cursor.execute("SELECT COUNT(*) FROM element_states")
            state_count = cursor.fetchone()[0]
            
            conn.close()
            
            print(f"Database: {elem_count} elements, {state_count} states")
        except Exception as e:
            print(f"Database: Error reading - {e}")
    else:
        print("Database: Not created yet")
    
    print()
    print("Next steps:")
    if not Path(rust_binary).exists():
        print("  1. python manage.py build      # Compile Rust program")
    if not Path(db_path).exists():
        print("  2. python manage.py record     # Record first element")
    else:
        print("  1. python manage.py verify     # Check integrity")
        print("  2. python manage.py list       # View elements")
        print("  3. python manage.py export ... # Export for mouse-clicker")

def main():
    if len(sys.argv) < 2:
        quick_status()
        print()
        print("Run 'python manage.py help' for all commands")
        return
    
    cmd = sys.argv[1].lower()
    
    if cmd in ['help', '--help', '-h']:
        print_help()
    elif cmd == 'status':
        quick_status()
    else:
        extra_args = sys.argv[2:] if len(sys.argv) > 2 else None
        run_command(cmd, extra_args)

if __name__ == "__main__":
    main()
