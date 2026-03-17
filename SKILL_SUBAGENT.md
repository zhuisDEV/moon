# MOON Sub-agent Skill (Memory Operations Only)

Use this skill when a sub-agent needs memory search/distill/embed functions only.

## Allowed Commands (Only These)

1. Search history:
`moon recall --name history_lib --query "<keywords>"`
2. L1 Normalisation (one projection file):
`moon distill -mode norm -file <path-to-projection-md> [-session-id <id>]`
3. L2 Synthesis (whole `memory.md` rewrite):
`moon distill -mode syns`
4. L2 Synthesis from explicit sources only:
`moon distill -mode syns -file <path> [-file <path> ...]`
5. Embed bounded batches:
`moon embed --name history_lib --max-docs <N>`

## Operating Rules

1. **Search first**: If the task depends on prior context, run `moon recall` before answering.
2. **Use bounded embed only**: `moon embed` must include `--max-docs <N>` to avoid unbounded runs.
3. **Minimal side effects**: Do not run loops/daemons; execute single-shot commands only.
4. **No hallucinated history**: If recall has no relevant results, explicitly report no hit.
5. **Run via installed binary**: Use `moon ...` commands only.
6. **No separate index command**: use `moon embed` to refresh searchable content when needed.

## Prohibited Commands

- Any build/runtime admin command: `cargo *`, `moon install`, `moon verify`, `moon repair`.
- Any lifecycle/process control command: `moon watch`, `moon stop`, `moon restart`.
- Any active-window control command: `moon record`, `moon cleanse`, `moon assemble`, `moon context-engine`.
- Any command not listed under "Allowed Commands (Only These)".

## Scope

- Target collection for recall/embed: `history_lib`.
- This skill is for sub-agents and least-privilege memory operations only.
