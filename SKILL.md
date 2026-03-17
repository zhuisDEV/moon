# M.O.O.N. Admin Skill

Use this skill for moon system admin/operator operations.
For least-privilege sub-agents, use `SKILL_SUBAGENT.md` instead.

This skill covers:
1. Admin/bootstrap lifecycle (`install`, `verify`, `repair`, `status`, `config`, `health`).
2. Moon v1 runtime workflows (`record`, `project`, `cleanse`, `assemble`, `context-engine`, `distill`, `recall`).
3. Search maintenance (`embed`).
4. Transitional runtime control (`watch`, `stop`, `restart`).
5. Real OpenClaw integration testing of the primary flow.

## Operating Rule

1. Use the installed Moon README at `$MOON_HOME/README.md` as the source of truth for setup, env vars, commands, safety flags, and uninstall. When running from source before install, use the repo `README.md`.
2. Always run from the repo root for source-mode commands. Path model: `MOON_HOME` is the Moon runtime root, the repo path is separate from `MOON_HOME`, and memory path is `$MOON_HOME/memory`. Runtime env loading is strict from `$MOON_HOME/.env` (not repo-local `.env`).
3. If the `moon` binary is installed in your `$PATH` (e.g. `~/.cargo/bin/moon`), run `moon <command>`. Otherwise, run `cargo run -- <command>` from the repo folder.
4. If you modify any Rust source code (`src/*.rs`) or plugin assets (`assets/plugin/*`), you MUST run `cargo install --path .` ONCE to compile and apply those changes.
5. Prefer JSON mode for automation: `moon --json <command>` or `cargo run -- --json <command>`.
6. For first-time setup and after OpenClaw upgrades, run `moon install` before `moon verify --strict`. `install` is responsible for engine/bootstrap shell wiring, runtime-root provisioning under `$MOON_HOME`, provenance self-heal (`plugins.installs.moon.*`), and, on macOS with installed binary, launchd auto-start wiring for the transitional watcher maintenance daemon (Windows/Linux: no autostart wiring yet).
7. Treat runtime provenance diagnostics as authoritative: if `moon status` or `moon verify --strict` reports `loaded without install/load-path provenance`, run `moon install` and re-check.
8. If `moon status` only prints `provenance repair hint` (without failing), it is non-fatal drift; run `moon install` to normalize.
9. `install`, `verify`, `repair`, `status`, `config`, and `health` should be interpreted as `moon-context-engine` admin/bootstrap commands, not as the final watcher-first control plane.
10. If `moon status` reports runtime or policy drift, fix with `moon install` (or `moon repair`) and re-check before continuing.
11. Use `moon recall` as the user-facing search command whenever prior Moon-managed context is needed.
12. Use `moon embed` as the public search-maintenance command. There is no separate public `moon index` command in Moon v1.
13. `moon embed` is Moon orchestration over qmd. Prefer collection-bounded
    embed when qmd supports `--max-docs`; on newer qmd builds that only expose
    global embed, Moon may use `qmd embed --max-docs-per-batch <n>` against
    Moon-owned qmd state under `$MOON_HOME/qmd/`.
14. Treat `moon-context-engine` as the short-lived normal-path controller for active context preparation.
15. Treat `watch`, `stop`, and `restart` as transitional long-running maintenance commands only; they are not the active-window controller.
16. For fallback operations, use OpenClaw plugin runtime config under `plugins.entries.moon.config`: `fallbackMode` (`openclaw` or `disabled`) and `compactFallbackOnSkip` (`true` or `false`).
17. Keep the primary flow first. Do not mix primary and fallback together during normal setup/testing.
18. Treat the lane split as intentional:
    - `mce` hot lane: immediate `raw -> project(mds/<collection>) -> embed(hot) -> cleanse -> assemble`
    - watcher library lane: maintenance `raw -> project(mlib) -> embed(history_lib) -> distill --mode norm`
19. `mce` and watcher run in parallel. `mce` immediate project/embed is not governed by watcher cooldown or cycle timing.
20. Hot collection lifecycle policy belongs in `$MOON_HOME/moon.toml` under `[hot_collection]`. Do not use removed env overrides for it.
21. Do not use removed/suspended config keys: `prune_mode`, `[retention]`, `[inbound_watch]`.
22. For real OpenClaw smoke tests, verify after `moon install` that OpenClaw config contains `moonHome`, `memoryDir`, and `memoryFile` under `plugins.entries.moon.config`.
23. Use `moon config --show` to confirm resolved cleanse thresholds. With the example config they are `100000` trigger and `180000` emergency.
24. Hot session projections live directly under `$MOON_HOME/mds/<collection>/`.
    Do not introduce an extra `mds/hot/` layer.
25. `moon install` exports runtime-owned docs into `$MOON_HOME` and exports role-scoped skills into `$OPENCLAW_STATE_DIR/skills/`, so installed operation does not depend on the repo checkout remaining on disk.

## Maintenance

If the daemon/runtime looks inconsistent, use this clean recovery sequence:

1. `launchctl bootout "gui/$(id -u)" "$HOME/Library/LaunchAgents/com.moon.watch.plist" 2>/dev/null || true`
2. `pkill -f "moon watch --daemon" 2>/dev/null || true`
3. `pkill -f "moon restart" 2>/dev/null || true`
4. `rm -f "$MOON_HOME/logs/moon-watch.daemon.lock" "$MOON_HOME/logs/moon-embed.lock" "$MOON_HOME/logs/l1-normalisation.lock"`
5. `moon install`
6. `moon verify --strict`
7. `moon health`
