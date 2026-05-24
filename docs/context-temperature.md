# Context Temperature Model

BRE separates two ideas that are often mixed together:

- **Temperature**: how relevant a file is to the current task right now.
- **Criticality**: how dangerous or globally important a file is if changed or misunderstood.

A file can be cold and critical at the same time. For example, a rarely touched environment config may be low-temperature but high-criticality.

## Buckets

### Hot

Files that should be loaded with exact source because they are directly involved in the current work.

Signals:

- currently edited
- explicitly mentioned by the user
- appears in a failing stack trace
- imported by a file being edited
- owns the failing test or broken route
- matches task terms strongly

### Warm

Files that are likely relevant but do not always need full source loaded immediately.

Signals:

- dependency neighbor of a hot file
- shares symbols, routes, models, or test fixtures with hot files
- recently changed on the branch
- referenced by linked docs or tickets
- contains interfaces used by hot files

### Cold

Files that are indexed and searchable but should not spend active context unless a reason appears.

Signals:

- no current task match
- far from edited files in the dependency graph
- no recent branch changes
- no test, stack trace, or ticket references

### Critical

Files that may have high blast radius even when they are not hot.

Signals:

- environment or deployment config
- package manager and build config
- routing middleware
- database schema or migrations
- authentication/session boundary
- payment/billing integration
- shared types used across packages
- CI and release workflow

Critical is not a temperature. It is an overlay.

## Scoring Sketch

BRE should not hide the ranking behind magic. A first version can use a simple explainable score:

```text
temperature_score(file) =
  direct_user_reference
+ current_edit_bonus
+ failing_trace_bonus
+ git_change_bonus
+ dependency_distance_score
+ task_term_match
+ linked_doc_match
- time_decay
```

Criticality can be scored separately:

```text
criticality_score(file) =
  config_boundary
+ auth_boundary
+ database_boundary
+ payment_boundary
+ build_or_deploy_boundary
+ shared_api_surface
```

The UI should show the reason, not only the score.

Example:

```text
src/auth/session.ts
Temperature: hot
Criticality: high
Why: edited in this task, imported by failing login test, owns session boundary.
```

## Adaptive Weights

BRE stores learned priority weights in `bre_weights.json`, but the learning loop
is deliberately bounded. When diagnostics show that a background file caused a
compiler error, BRE boosts only the relevant scoring signals within a fixed
headroom above their defaults. When a run is clean, those weights decay back
toward the default baseline.

This prevents one long refactor from permanently inflating every signal until
the tray can no longer distinguish an AST dependency from a manifest, dirty
file, or symbol match.

## Context Tray

Every agent task should expose a context tray:

```text
Active Context:
- exact source loaded now

Watching:
- cold but critical files
- dependency neighbors
- docs that might invalidate assumptions

Indexed:
- searchable repo background

Pinned:
- files or docs the user explicitly wants considered
```

## Invalidation

The context tray must change when reality changes.

Invalidation triggers:

- user changes the task
- branch receives new edits
- tests fail in a new area
- a dependency edge changes
- a linked ticket or doc changes
- agent assumptions conflict with observed behavior

When invalidated, BRE should update the tray and explain what changed.

## Prototype Goal

The first prototype should answer one question:

> Given a human task and a repo, can BRE produce a better context set than loading recently changed files only?

That is the smallest serious proof of the product.
