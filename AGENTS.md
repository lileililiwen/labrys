# Labrys agent entry point

Labrys is an agent-native, technology-agnostic Application PaaS. The active
implementation queue is in OpenSpec and is ordered by `ROADMAP.md`.

Required workflow:

```text
openspec list → select one change → BFS impact map → structural pass
→ DFS requirement implementation → local verification/Gate
→ BFS regression/completeness review → strict validation → archive
→ commit 1 (implementation, tests, archive, specs)
→ update HANDOFF.md → commit 2 (handoff pointer only)
```

- Read `HANDOFF.md` before work and maintain exactly one `current_spec` pointer.
- Each spec change ends with exactly two commits: commit 1 holds the
  implementation, tests, archived change, and promoted specs; commit 2 is the
  separate `HANDOFF.md` pointer update to the next change. Stop after commit 2.
- Non-trivial work requires an OpenSpec change with proposal, design, tasks,
  and scenario-based capability specs.
- Read `.ai-rules/workflow.md` for the full lifecycle and `.ai-rules/completion.md`
  for stopping conditions.
- Read `.ai-rules/architecture.md` for new modules, public abstractions,
  persistence, controllers, providers, runtimes, or integrations.
- Project-specific gate declarations are in `.ai-gate/gate.yaml`; shared gate
  runtimes and universal rule packs stay outside this repository.
- Run `node scripts/check-openspec-change-names.mjs` before OpenSpec status,
  instructions, strict validation, or archive.
- Build success, test success, strict spec validation, or skeleton completion
  are not by themselves DONE.
- Preserve unrelated worktree changes and stage only files related to the
  selected change.

