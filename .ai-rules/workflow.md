# Labrys change workflow

## BFS → DFS → BFS

1. **BFS impact map:** inspect requirements, domain concepts, modules,
   contracts, callers, persistence, events, security, compatibility, tests,
   observability, and evidence boundaries before editing deeply.
2. **Structural pass:** update domain types, protocols, DTOs, events, wiring,
   migrations, callers, and test skeletons. Label compile-only progress
   `SKELETON_READY`; it is not behavior completion.
3. **DFS implementation:** implement one coherent requirement/scenario through
   domain, application, infrastructure, and integration layers.
4. **BFS verification:** re-check every requirement and scenario, callers,
   persistence, APIs, authorization, validation, logging, events, concurrency,
   compatibility, placeholders, tests, and Gate evidence.

## OpenSpec lifecycle

Run `openspec list`, choose one dependency-ready active change, and set exactly
one `current_spec: <name>` line in `HANDOFF.md`. Work only on that change and
its tests. Update `tasks.md` from evidence. Run local verification and the
applicable Gate before strict validation. Archive only fully verified changes;
promote canonical specs and never use `--skip-specs`. Commit implementation,
tests, archive, and related specs, then update and separately commit the
handoff pointer. Stop after the handoff update.

## Blocking preflight

Before `openspec status --change`, `openspec instructions --change`, strict
validation, or archive, run:

```bash
node scripts/check-openspec-change-names.mjs
```

It rejects active change directories that do not match
`^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$`.

## Platform invariants

Application is the top-level object. Imported and generated projects share the
same model. Platform API precedes framework tools, generic tools, and raw
shell. Secrets stay outside LLM context. Production destructive operations
require human approval by default. Desired state and actual state are separate
and reconciled. Independent verification is required for agent claims.

