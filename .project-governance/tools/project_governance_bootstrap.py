#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Project Governance Bootstrap v4.1 - Fully English Clean Edition
- All console output, file contents, comments, and instructions in English.
- Creates SYSTEM_PROMPT.md, AGENTS.md, OWNER_VISION.md, and PROJECT_MAP.md upon init.
- Supports external paths via --root.
"""
import json, re, sys, shutil, os
from pathlib import Path
from datetime import datetime, timezone

TOOL_VERSION = "4.1-clean-en"
GOV_DIR = ".project-governance"
TOOLS_DIR = "tools"
FILES = {
    "requirements": "requirements.json",
    "problems": "problems.json",
    "proposals": "proposals.json",
    "criticisms": "criticisms.json",
    "project_map": "PROJECT_MAP.md",
    "owner_vision": "OWNER_VISION.md",
    "working_context": "WORKING_CONTEXT.md",
    "system_prompt": "SYSTEM_PROMPT.md",
    "event_log": "event_log.jsonl",
}
EXCLUDED_DIRS = {".git",".hg",".svn",".idea",".venv","venv","env","node_modules","target","dist","build","out","__pycache__",".mypy_cache",".project-governance","bin","obj",".next",".turbo"}
EXCLUDED_NAMES = {".env",".env.local","credentials.json","secrets.json","id_rsa"}

def now(): return datetime.now(timezone.utc).isoformat()[:19]+"Z"
def norm(s): return re.sub(r'\s+', ' ', s.strip().lower())

def load_json_list(root, key):
    p = root / GOV_DIR / FILES[key]
    if not p.exists(): return []
    try: return json.loads(p.read_text(encoding='utf-8'))
    except: return []

def save_json_list(root, key, data):
    p = root / GOV_DIR / FILES[key]
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding='utf-8')

def log(root, action, data):
    p = root / GOV_DIR / FILES["event_log"]
    p.parent.mkdir(parents=True, exist_ok=True)
    rec={"time":now(),"action":action,**data}
    with p.open("a",encoding="utf-8") as f: f.write(json.dumps(rec,ensure_ascii=False)+"\n")

def next_id(items, prefix):
    if not items: return f"{prefix}-0001"
    nums=[]
    for it in items:
        m=re.search(r'(\d+)', it.get('id',''))
        if m: nums.append(int(m.group(1)))
    return f"{prefix}-{max(nums)+1:04d}" if nums else f"{prefix}-0001"

# ===== PROJECT MAP - Smart inspect =====
EXECUTABLE_EXTS = {".rs",".py",".ts",".tsx",".js",".jsx",".go",".java",".kt",".cs",".cpp",".c",".h",".hpp",".swift",".php",".rb",".dart",".sh"}
EXECUTABLE_NAMES = {"cargo.toml","package.json","tauri.conf.json","pyproject.toml","go.mod","pom.xml","build.gradle","makefile"}

def walk_project(root, max_entries=500):
    files=[]
    for cur, dirs, fnames in os.walk(root, topdown=True):
        cp=Path(cur)
        dirs[:]=[d for d in dirs if d not in EXCLUDED_DIRS and not d.startswith(".") or d in {".vscode",".github",".clinerules"}]
        for fn in sorted(fnames):
            p=cp/fn
            if fn in EXCLUDED_NAMES: continue
            suf=p.suffix.lower()
            is_executable = suf in EXECUTABLE_EXTS or fn.lower() in EXECUTABLE_NAMES
            if not is_executable: continue
            if p.stat().st_size>800_000: continue
            try: rel=p.relative_to(root).as_posix()
            except: rel=p.as_posix()
            if rel.startswith(".project-governance/"): continue
            files.append(p)
            if len(files)>=max_entries: return files, True
    return files, False

def smart_describe_file(path, root):
    name=path.name.lower()
    suf=path.suffix.lower()
    try: parent=path.parent.relative_to(root).as_posix()
    except: parent="root"
    
    content = ""
    try: content = path.read_text(encoding='utf-8', errors='ignore')[:3000]
    except: content = ""
    
    if name=="cargo.toml": return "Rust Config", "Defines dependencies and package parameters"
    if name=="package.json": return "Node Config", "Dependencies, scripts, and package settings"
    if name=="main.rs": return "Main Entry Point", "Application bootstrap and runtime initialization"
    
    fns = re.findall(r'fn\s+(\w+)', content)
    comps = re.findall(r'(?:function|const|class)\s+(\w+)', content)
    
    if suf in {".rs"}: return f"Rust Code | {parent}", f"Key functions: {', '.join(fns[:4])}" if fns else "Source executable"
    elif suf in {".ts", ".tsx"}: return f"TypeScript | {parent}", f"Exports: {', '.join(comps[:3])}" if comps else "UI / logic file"
    elif suf == ".py": return f"Python Script | {parent}", "Python script or module"
    else: return f"Executable {suf} | {parent}", "Source code file"

def detect_stack(root, files):
    names = {p.name.lower() for p in files}
    suffixes = {p.suffix.lower() for p in files}
    stack = []
    if "cargo.toml" in names or ".rs" in suffixes: stack.append("Rust")
    if "package.json" in names or suffixes & {".js", ".jsx", ".ts", ".tsx"}: stack.append("JavaScript/TypeScript")
    if "pyproject.toml" in names or "requirements.txt" in names or ".py" in suffixes: stack.append("Python")
    if "tauri.conf.json" in names: stack.append("Tauri")
    if "go.mod" in names or ".go" in suffixes: stack.append("Go")
    return list(dict.fromkeys(stack)) or ["Undefined Stack"]

def generate_project_map(root):
    files, truncated = walk_project(root)
    stack = detect_stack(root, files)
    lines=[]
    lines.append("# PROJECT MAP - Current Repository Evidence Map")
    lines.append(f"_Generated: {now()} by {TOOL_VERSION}_")
    lines.append(f"_Root Path: {root}_")
    lines.append("")
    lines.append("## 🧰 Detected Stack")
    for s in stack: lines.append(f"- {s}")
    lines.append("")
    lines.append("## ⚡ Source & Config Files")
    lines.append(f"Total Files Scanned: {len(files)}")
    lines.append("")
    lines.append("| Path | Summary | Description |")
    lines.append("|---|---|---|")
    
    for p in sorted(files, key=lambda x: (len(x.parts), x.as_posix())):
        try: rel=p.relative_to(root).as_posix()
        except: rel=p.name
        short, detail = smart_describe_file(p, root)
        lines.append(f"| `{rel}` | {short} | {detail} |")
        
    if truncated:
        lines.append("\n> Scan limited to 500 executable files.")
        
    lines.append("\n## 📂 Governance Directory Structure")
    lines.append("| File | Role |")
    lines.append("|---|---|")
    lines.append("| requirements.json | Active tasks - deleted upon completion |")
    lines.append("| problems.json | Active bugs or obstacles |")
    lines.append("| proposals.json | Proposed additions requiring approval |")
    lines.append("| criticisms.json | Architecture or code quality concerns |")
    lines.append("| PROJECT_MAP.md | Generated repository file index |")
    lines.append("| OWNER_VISION.md | Read-only owner intent and constraints |")
    lines.append("| SYSTEM_PROMPT.md | Project-independent AI instructions |")
    
    return "\n".join(lines)

def install_self_copy(root):
    source=Path(__file__).resolve()
    target_dir=root / GOV_DIR / TOOLS_DIR
    target_dir.mkdir(parents=True, exist_ok=True)
    target=target_dir / "project_governance_bootstrap.py"
    try:
        if source==target.resolve(): return "already in place"
    except: pass
    shutil.copy2(source, target)
    return f"installed to {target}"

def init_project(root):
    g=root / GOV_DIR
    g.mkdir(exist_ok=True)
    copy_msg=install_self_copy(root)
    
    # 1. OWNER_VISION.md
    ov=g / FILES["owner_vision"]
    if not ov.exists():
        ov.write_text("""# Owner Vision (owner-controlled)
This file is controlled by the project owner. The AI may read it but MUST NOT edit, rewrite, reorganize, or append to it.

## Project identity
- Project name: [Specify Name]
- Domain: [Specify Domain]

## Project purpose
[Describe core requirements and target goals for this project]

## Priorities
1. Correctness
2. Performance
3. Simplicity

## Constraints
- Do not introduce unnecessary dependencies or over-engineer.
""", encoding='utf-8')

    for key in ["requirements","problems","proposals","criticisms"]:
        p=g / FILES[key]
        if not p.exists(): save_json_list(root, key, [])

    # 2. PROJECT_MAP.md
    pm=g / FILES["project_map"]
    pm.write_text(generate_project_map(root), encoding='utf-8')

    # 3. WORKING_CONTEXT.md
    wc=g / FILES["working_context"]
    if not wc.exists():
        wc.write_text(f"# Working Context\n\n## Current Goal\nInitialize project development.\n\n## Last Update\n{now()}\n", encoding='utf-8')

    # 4. SYSTEM_PROMPT.md (Instructions for AI)
    sp=g / FILES["system_prompt"]
    sp.write_text("""# Generic Project Governance Agent

You are an engineering assistant working in the repository that contains `.project-governance/`.
This system tracks tasks without relying on external assumptions. Never assume past project context, stacks, or features without explicit repository evidence.

## Governance Files
- `OWNER_VISION.md`: Read-only source of owner intent.
- `PROJECT_MAP.md`: Generated repository index.
- `requirements.json`: Approved active work items.
- `problems.json`: Confirmed blockers or bugs.
- `proposals.json`: Optional improvements requiring approval.
- `criticisms.json`: Concerns regarding design, security, or quality.

## Mandatory Workflow
1. Check status: `python3 .project-governance/tools/project_governance_bootstrap.py status`
2. Inspect `OWNER_VISION.md` and active tasks in `requirements.json`.
3. Implement active requirements one by one.
4. When a requirement is completed and verified, mark it done: `python3 .project-governance/tools/project_governance_bootstrap.py done REQ-XXXX`
5. Automatically add user feature requests or bugs to governance using `new requirement` or `new problem`.
""", encoding='utf-8')

    # 5. AGENTS.md
    agents=root / "AGENTS.md"
    agents.write_text("""# Project Governance Rules
Before starting work, read `.project-governance/SYSTEM_PROMPT.md`, `OWNER_VISION.md`, and `PROJECT_MAP.md`.
Use the helper script `.project-governance/tools/project_governance_bootstrap.py` to keep tasks and project maps updated.
""", encoding='utf-8')

    print(f"[SUCCESS] Governance initialized at: {g}")
    print(f"   {copy_msg}")
    return 0

def add_item(root, kind, title, desc=""):
    map_prefix={"requirement":"REQ","problem":"PROB","proposal":"PROP","criticism":"CRIT"}
    prefix=map_prefix.get(kind, "REQ")
    file_key=kind+"s"
    if kind=="criticism": file_key="criticisms"
    elif kind=="requirement": file_key="requirements"
    elif kind=="problem": file_key="problems"
    elif kind=="proposal": file_key="proposals"
    
    items=load_json_list(root, file_key)
    all_items=[]
    for fk in ["requirements","problems","proposals","criticisms"]: all_items.extend(load_json_list(root, fk))
    
    nt=norm(title)
    for it in all_items:
        if norm(it['title'])==nt:
            print(f"[DUPLICATE ERROR] {it['id']} already exists: {it['title']}")
            return 2
            
    new_id=next_id(items, prefix)
    item={"id":new_id, "title":title.strip(), "description":desc.strip(), "created_at":now(), "status":"open", "type":kind}
    items.append(item)
    save_json_list(root, file_key, items)
    log(root, f"new_{kind}", {"id":new_id, "title":title})
    print(f"[ADDED] {new_id} [{kind}] - {title}")
    return 0

def done_item(root, item_id):
    item_id=item_id.upper()
    for fk in ["requirements","problems","proposals","criticisms"]:
        items=load_json_list(root, fk)
        found=None; rest=[]
        for it in items:
            if it['id'].upper()==item_id: found=it
            else: rest.append(it)
        if found:
            save_json_list(root, fk, rest)
            log(root, "done_and_deleted", {"id":found['id'], "from":fk})
            print(f"[COMPLETED & REMOVED] {found['id']} from {fk}")
            return 0
    print(f"[NOT FOUND] Item: {item_id}")
    return 2

def promote_item(root, old_id, new_title):
    old_id=old_id.upper()
    for fk in ["criticisms","problems","proposals"]:
        items=load_json_list(root, fk)
        found=None; rest=[]
        for it in items:
            if it['id'].upper()==old_id: found=it
            else: rest.append(it)
        if found:
            save_json_list(root, fk, rest)
            log(root, "promoted", {"from":old_id, "to":new_title})
            print(f"[PROMOTED] {old_id} converted to requirement.")
            return add_item(root, "requirement", new_title, found.get('description',''))
    print(f"[NOT FOUND] ID: {old_id}")
    return 2

def status(root):
    print("\n=== Governance Status ===")
    g=root / GOV_DIR
    reqs=load_json_list(root, "requirements")
    probs=load_json_list(root, "problems")
    props=load_json_list(root, "proposals")
    crits=load_json_list(root, "criticisms")
    print(f"Location: {g}/")
    print(f"Counts -> Requirements: {len(reqs)} | Problems: {len(probs)} | Proposals: {len(props)} | Criticisms: {len(crits)}")
    if reqs:
        print("\n[ACTIVE REQUIREMENTS]:")
        for it in reqs[:5]: print(f"   - {it['id']}: {it['title']}")
    else:
        print("\n[INFO] No active requirements.")
    return 0

def map_cmd(root):
    g=root / GOV_DIR
    content=generate_project_map(root)
    pm=g / FILES["project_map"]
    pm.write_text(content, encoding='utf-8')
    log(root, "map_refreshed", {})
    print("[SUCCESS] PROJECT_MAP.md updated successfully.")
    return 0

def validate(root):
    all_items=[]
    for k in ["requirements","problems","proposals","criticisms"]: all_items.extend(load_json_list(root, k))
    titles={}; ids={}; ok=True
    for it in all_items:
        nt=norm(it['title'])
        if nt in titles:
            print(f"[DUPLICATE TITLE] {it['id']} and {titles[nt]}")
            ok=False
        titles[nt]=it['id']
        if it['id'] in ids:
            print(f"[DUPLICATE ID] {it['id']}")
            ok=False
        ids[it['id']]=1
    if ok: print(f"[VALID] Clean state - {len(all_items)} total items.")
    return 0 if ok else 1

def main():
    import argparse
    p=argparse.ArgumentParser(description="Clean English Governance Tool")
    p.add_argument("--root", default=".", help="Target project root path")
    sub=p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("init", help="Initialize clean governance structure")
    a=sub.add_parser("new", help="Add new item")
    a.add_argument("kind", choices=["requirement","problem","proposal","criticism"])
    a.add_argument("title")
    a.add_argument("--desc", default="")
    d=sub.add_parser("done", help="Complete and remove item")
    d.add_argument("id")
    pr=sub.add_parser("promote", help="Promote item to requirement")
    pr.add_argument("old_id")
    pr.add_argument("new_title")
    sub.add_parser("status", help="Show status summary")
    sub.add_parser("validate", help="Check integrity")
    sub.add_parser("map", help="Refresh repository map")
    args=p.parse_args()
    
    root=Path(args.root).resolve()
    if args.cmd=="init": return init_project(root)
    if args.cmd=="new": return add_item(root, args.kind, args.title, args.desc)
    if args.cmd=="done": return done_item(root, args.id)
    if args.cmd=="promote": return promote_item(root, args.old_id, args.new_title)
    if args.cmd=="status": return status(root)
    if args.cmd=="validate": return validate(root)
    if args.cmd=="map": return map_cmd(root)

if __name__=="__main__":
    sys.exit(main())