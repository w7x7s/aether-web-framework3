# Generic Project Governance Agent

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
