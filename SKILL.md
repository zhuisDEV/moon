# M.O.O.N. Runtime Skill (Minimal CLI)

Use this skill for the minimum runtime operations needed to run Moon safely.
For least-privilege sub-agent usage, use `SKILL_SUBAGENT.md`.

For full operational details, edge cases, and troubleshooting, use:
`$MOON_HOME/README.md`.

## Runtime Assumptions

1. `MOON_HOME` points to your runtime root (default: `$HOME/.moon`).
2. Runtime env file is `$MOON_HOME/.env`.
3. Use `moon update` for normal upgrades; it runs install+verify and preserves
   `$MOON_HOME/.env` and `$MOON_HOME/moon.toml`.

## Simple Agent Guide

1. Prepare runtime:
   `moon install`
   For upgrades:
   `moon update`
2. Validate wiring:
   `moon verify --strict`
3. Confirm health:
   `moon status`
   `moon health`
4. Run normal checkpoint path:
   `moon context-engine --used-tokens <N> --max-tokens <M>`
5. Use memory/search as needed:
   `moon recall --name history_lib --query "<query>"`
   `moon embed --name history_lib --max-docs <N>`
6. If daemon maintenance is required:
   `moon watch --once`
   `moon restart` or `moon stop`

If anything is unclear or fails unexpectedly, go to `$MOON_HOME/README.md`
first.

## Minimum Runtime CLI

1. Bootstrap runtime wiring:
   - `moon install`
2. Verify plugin/runtime wiring:
   - `moon verify --strict`
3. Inspect resolved runtime state:
   - `moon status`
   - `moon config --show`
4. Check daemon + state health:
   - `moon health`
5. Run primary path checkpoint:
   - `moon context-engine --used-tokens <N> --max-tokens <M>`
6. Run explicit stage commands (when needed):
   - `moon record`
   - `moon cleanse`
   - `moon assemble`
   - `moon project`
7. Search and memory operations:
   - `moon recall --name history_lib --query "<query>"`
   - `moon embed --name history_lib --max-docs <N>`
   - `moon distill -mode norm`
   - `moon distill -mode syns`
8. Transitional maintenance worker:
   - `moon watch --once`
   - `moon watch --daemon`
   - `moon stop`
   - `moon restart`

## Default Runtime Sequence

1. `moon install`
2. `moon verify --strict`
3. `moon status`
4. `moon health`
5. `moon context-engine --used-tokens <N> --max-tokens <M>`
