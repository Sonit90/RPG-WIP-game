# Agent workflow

The main session runs on **Opus** and acts as architect, orchestrator and reviewer.
Worker subagents (`.claude/agents/`) do the legwork:

| Agent | Model | Use for |
|---|---|---|
| `scout` | haiku | Read-only search: locate code, call sites, scene JSON entries |
| `runner` | haiku | Run build/test/clippy/bench; return compact pass/fail + exact errors |
| `implementer` | sonnet | Execute a concrete, scoped implementation plan |

## Orchestrator rules (main session)

- Delegating to these agents is pre-authorized for this project.
- Do design yourself: read the relevant code, decide the approach, write the plan. Don't delegate architecture decisions.
- Delegate broad searches to `scout` and noisy build/test runs to `runner` to keep main context clean. Small lookups (one file, one grep) — do directly.
- Hand `implementer` a self-contained brief: goal, files to touch, exact approach, constraints, acceptance criteria (which build/tests must pass). Workers start with no conversation context.
- Split independent work into parallel `implementer` tasks only when they touch disjoint files; otherwise run sequentially. Use `isolation: "worktree"` for risky or parallel overlapping work.
- **Always review** worker output: read the actual `git diff`, check against the plan, correctness, and surrounding style. Fix small issues directly; send larger rework back to `implementer` with specific feedback.
- Final verification (build + relevant tests) via `runner` before reporting done.
- Trivial edits (a few lines) — do directly; spawning costs more than it saves.
- **At most 3 agents running at once** (any mix of `scout`, `runner`, `implementer`). Before
  launching another, wait for one to finish. Queue extra work instead of spawning it.
- `isolation: "worktree"` bases worktrees on `master`, not the current branch. For parallel work
  create worktrees yourself (`git worktree add -b <branch> ../myGame-wt-<name> <current-branch>`),
  pass the absolute path in the brief, and remove the worktree after merging (each build dir is ~10 GB).
