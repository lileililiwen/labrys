# Labrys completion rules

A change may be called complete only when all applicable requirements and
observable scenarios are implemented, migrated callers and persistence are
verified, required tests exist and pass, the local Gate has run, and the final
BFS review covers the original impact surface.

Before archive or completion:

1. Run `node scripts/check-openspec-change-names.mjs`.
2. Run the repository's applicable format, lint, build, unit, integration,
   security, packaging, and runtime checks.
3. Run the applicable shared Gate selected by `.ai-gate/gate.yaml`.
4. Run `openspec validate --all --strict`.
5. Confirm no task, scenario, placeholder, unresolved `REVIEW_REQUIRED`, or
   blocking `FAIL` remains.
6. Inspect the staged diff and verify unrelated work is untouched.

Planning documents, compilation, strict OpenSpec validation, a skeleton, or a
successful isolated unit test do not prove runtime delivery. `BLOCKED` is not
`PASS`. If work is incomplete, record the exact failed command, evidence, and
next action in `HANDOFF.md`; do not claim completion.

