# Design

Each Agent Session receives an isolated worktree from the canonical repository.
Changes flow through build, preview, human review, and merge. Web previews use
a reverse proxy and temporary URL. Expo projects use a cloud development
runtime and QR flow, starting with Expo Go and leaving development builds for a
later maturity level. Preview environments are disposable and environment
scoped.

