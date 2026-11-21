# Getting started with Moon

Moon is OpenClaw's persistent memory store. Your chat model and login stay in
OpenClaw. Already have Moon? Use the
[upgrade checklist](updating.md#upgrade-checklist); do not run a fresh
installation over an existing runtime.

## Before you install

You need OpenClaw **2026.9.2 or newer**, with a working model route and login.
The source installation below also needs Git, Rust stable and Cargo. Signed
releases support macOS 13+ on Apple Silicon/Intel and GNU/Linux arm64/x86_64
with glibc 2.35+. Deno is needed only for developer tests.

```bash
openclaw --version
openclaw config validate
```

Confirm a normal OpenClaw chat already works. Moon does not add another Codex
login or require `OPENAI_API_KEY`. Chat success does not establish that an
isolated learning helper can authenticate; verify learning separately.

## First installation from source

There is no unattended bootstrap installer. This source-build path needs a Rust
toolchain once; later signed updates do not. Maintainers can instead perform a
controlled [signed bootstrap](updating.md#v210-bootstrap-boundary).

Clone the release into a new directory and build:

```bash
git clone --branch v2.9.2 --depth 1 https://github.com/zhuisDEV/moon.git moon-2.9.2
cd moon-2.9.2
cargo build --locked --release
target/release/moon --json --version
```

Check version `2.9.2`, the expected target/commit and `git_dirty: false`. This
block refuses an existing runtime and installs a versioned layout:

```bash
sh <<'SH'
set -eu
if [ -e "$HOME/.moon" ] || [ -L "$HOME/.moon" ]; then
  echo 'Moon already exists; use the upgrade guide instead.' >&2
  exit 1
fi
moon_release="$HOME/.moon/releases/2.9.2"
install -d -m 700 "$HOME/.moon" "$HOME/.moon/releases" "$moon_release" \
  "$moon_release/bin" "$moon_release/openclaw-plugin" "$moon_release/skill" \
  "$moon_release/skill/docs" "$HOME/.moon/bin"
install -m 755 target/release/moon "$moon_release/bin/moon"
install -m 644 assets/openclaw-plugin/index.js assets/openclaw-plugin/package.json \
  assets/openclaw-plugin/openclaw.plugin.json assets/openclaw-plugin/README.md \
  "$moon_release/openclaw-plugin/"
install -m 644 SKILL.md "$moon_release/skill/SKILL.md"
install -m 644 docs/*.md "$moon_release/skill/docs/"
ln -s releases/2.9.2 "$HOME/.moon/current"
ln -s ../current/bin/moon "$HOME/.moon/bin/moon"
ln -s current/openclaw-plugin "$HOME/.moon/openclaw-plugin"
SH
```

Add the command to `PATH` (also persist this line in your shell startup file),
and initialise the new database explicitly:

```bash
export PATH="$HOME/.moon/bin:$PATH"
hash -r
command -v moon
moon --home "$HOME/.moon" --database "$HOME/.moon/state/moon.sqlite" init
moon --json health
```

Expect schema `21`, integrity `ok` and no logical violations. If a different
`moon` is found, repair `PATH` first. Never follow an old binary's migration
prompt against a newer database.

## Connect OpenClaw

Back up the configuration file reported by `openclaw config file` to a dated
file with mode `0600`. These settings make Moon the context/memory owner and
grant its recall hook conversation access. Apply only to the intended profile:

```bash
openclaw plugins install --link "$HOME/.moon/openclaw-plugin"
openclaw config set --batch-json '[
  {"path":"plugins.entries.moon.enabled","value":true},
  {"path":"plugins.entries.moon.config","value":{
    "moonPath":"~/.moon/bin/moon",
    "moonHome":"~/.moon",
    "mode":"hybrid",
    "maxChars":8000,
    "embeddingEnabled":true
  }},
  {"path":"plugins.entries.moon.hooks.allowConversationAccess","value":true},
  {"path":"plugins.entries.moon.hooks.allowPromptInjection","value":true},
  {"path":"plugins.slots.contextEngine","value":"moon"},
  {"path":"plugins.slots.memory","value":"none"},
  {"path":"agents.defaults.memorySearch.enabled","value":false}
]'
openclaw config validate
openclaw plugins doctor
openclaw gateway restart --safe
```

The explicit 8,000 cap is the source/topic recommendation; omitting `maxChars`
retains the compiled 3,500 default. First hybrid use downloads the local
embedding model and may take longer. OpenClaw retains transcript compaction;
local summarisation is a [separate option](compaction.md).

Install the agent instructions and guides with
[Refresh the agent skill](updating.md#refresh-the-agent-skill).

## Verify the installation

```bash
moon --json --version
moon --json health
openclaw gateway status
openclaw plugins info moon --json
moon metrics summary --since 7d
```

OpenClaw's `moonPath` and `command -v moon` should resolve to the same release.
An empty new database is normal. Health should report integrity `ok`, zero
logical violations and, once embeddings drain, no pending/failed/dead jobs.

For one-to-one Discord recall with native memory disabled, use the
`per-account-channel-peer` DM scope. Test with a fresh DM and a question that
omits the answer to a known owner-only fact. Check that the DM's Moon context
contains the memory and a room with an additional reader excludes it. A fact
already visible in channel history can also reach the model through OpenClaw.
Host filesystem, shell and cross-session tools have separate permissions. See
[Public and private memory](memory-privacy.md).

Storage health does not prove a chat received memory. Follow the
[native recall canary](openclaw-canary.md#native-recall-compatibility) with a
unique synthetic fact, a new chat and fresh injection evidence. The
[source-learning canary](source-learning-canary.md) separately checks real L1/L2
extraction and synthesis in a temporary home. It defaults to **no inference**;
explicit model and review stages consume provider usage. Its private host bridge
stops on unreviewed OpenClaw versions.

| Symptom                                | Next check                                                                |
| -------------------------------------- | ------------------------------------------------------------------------- |
| Wrong version or schema                | Compare executable paths; do not run `init` as a repair                   |
| Healthy plugin, no native recall       | Context-engine slot, hook permissions and the model route on a fresh turn |
| Recall works, no new memories          | L1 enabled state and fixed model failure codes                            |
| No daily reconciliation                | L2 defaults to disabled; inspect schedule and timezone                    |
| Source/topic missing                   | Acceptance, current revision, withdrawal, expiry and scope                |
| Setup, authentication or route failure | Fix the host, then explicitly retry the job                               |
| Interrupted update                     | Preserve the journal and follow [recovery](updating.md#apply)             |

Share only redacted, content-free diagnostics when asking for
[support](../SUPPORT.md).

## Configure learning and capture a source

Read [Learning](learning.md) before creating `moon.toml`. `config init` refuses
overwrite and creates a starter with L2 disabled. It neither checks model
availability nor logs in. Choose an account-available provider-qualified model;
L1 and L2 can use different models and efforts with the same host
authentication.

For articles and notes, follow [Source storage](source-storage.md), then
[Reviewed topics](topics.md). Reuse the same temporary home and scope throughout
the trial. Capture, proposal and acceptance are separate steps. Preserve
qualifiers and contradictions during review. History is retained until explicit
deletion; withdrawal disables support immediately.
