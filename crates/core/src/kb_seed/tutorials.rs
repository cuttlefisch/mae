use mae_kb::{KnowledgeBase, Node, NodeKind};

/// Install the progressive getting-started tutorial nodes.
pub(super) fn install_tutorial_nodes(kb: &mut KnowledgeBase) {
    let nodes = vec![
        // Hub
        Node::new(
            "tutorial:getting-started",
            "Getting Started with MAE",
            NodeKind::Tutorial,
            TUTORIAL_GETTING_STARTED,
        )
        .with_tags(["tutorial"]),
        // Vim track
        Node::new(
            "tutorial:vim-familiar",
            "Tutorial: What Carries Over from Vim",
            NodeKind::Tutorial,
            TUTORIAL_VIM_FAMILIAR,
        )
        .with_tags(["tutorial", "vim"]),
        Node::new(
            "tutorial:vim-differences",
            "Tutorial: What's Different from Vim",
            NodeKind::Tutorial,
            TUTORIAL_VIM_DIFFERENCES,
        )
        .with_tags(["tutorial", "vim"]),
        // Beginner track
        Node::new(
            "tutorial:what-is-modal",
            "Tutorial: What Is Modal Editing?",
            NodeKind::Tutorial,
            TUTORIAL_WHAT_IS_MODAL,
        )
        .with_tags(["tutorial", "beginner"]),
        Node::new(
            "tutorial:basic-movement",
            "Tutorial: Basic Movement",
            NodeKind::Tutorial,
            TUTORIAL_BASIC_MOVEMENT,
        )
        .with_tags(["tutorial", "beginner"]),
        Node::new(
            "tutorial:basic-editing",
            "Tutorial: Basic Editing",
            NodeKind::Tutorial,
            TUTORIAL_BASIC_EDITING,
        )
        .with_tags(["tutorial", "beginner"]),
        // Shared convergence nodes
        Node::new(
            "tutorial:mae-navigation",
            "Tutorial: MAE Navigation",
            NodeKind::Tutorial,
            TUTORIAL_MAE_NAVIGATION,
        )
        .with_tags(["tutorial"]),
        Node::new(
            "tutorial:mae-extending",
            "Tutorial: Extending MAE",
            NodeKind::Tutorial,
            TUTORIAL_MAE_EXTENDING,
        )
        .with_tags(["tutorial"]),
        // AI track
        Node::new(
            "tutorial:ai-setup",
            "Tutorial: AI Setup",
            NodeKind::Tutorial,
            TUTORIAL_AI_SETUP,
        )
        .with_tags(["tutorial", "ai"]),
        Node::new(
            "tutorial:ai-agent",
            "Tutorial: AI Agent (Terminal)",
            NodeKind::Tutorial,
            TUTORIAL_AI_AGENT,
        )
        .with_tags(["tutorial", "ai"]),
        Node::new(
            "tutorial:ai-chat",
            "Tutorial: AI Chat (Built-in)",
            NodeKind::Tutorial,
            TUTORIAL_AI_CHAT,
        )
        .with_tags(["tutorial", "ai"]),
        // Collaboration track
        Node::new(
            "tutorial:collab-setup",
            "Tutorial: Collaborative Setup",
            NodeKind::Tutorial,
            TUTORIAL_COLLAB_SETUP,
        )
        .with_tags(["tutorial", "collaboration", "sync"]),
    ];

    for node in nodes {
        kb.insert(node);
    }
}

// --- Tutorial content ---

const TUTORIAL_GETTING_STARTED: &str = "\
# Getting Started with MAE\n\n\
MAE (Modern AI Editor) is an AI-native Lisp machine editor with modal editing.\n\n\
Choose your track:\n\n\
## I know Vim\n\
→ [[tutorial:vim-familiar|Start here]] — what carries over, what's different, MAE extensions\n\n\
## I'm new to modal editing\n\
→ [[tutorial:what-is-modal|Start here]] — what modes are, basic movement, basic editing\n\n\
## Set up AI\n\
→ [[tutorial:ai-setup|AI Setup]] — API key configuration, provider selection, agent vs chat\n\n\
## Collaborate\n\
→ [[tutorial:collab-setup|Collaborative Setup]] — share buffers and KBs across machines\n\n\
Each track is a linked sequence of short lessons. Follow the **Next:** links at the bottom.\n\n\
See also: [[tutor:index|Lesson-style Tutorial]], [[index|Help Index]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_VIM_FAMILIAR: &str = "\
# What Carries Over from Vim\n\n\
If you know Vim, you already know most of MAE. These all work as expected:\n\n\
## Movement\n\
`h`/`j`/`k`/`l`, `w`/`b`/`e`, `gg`/`G`, `0`/`$`, `Ctrl-d`/`Ctrl-u`, `f`/`t`, `%`\n\n\
## Modes\n\
Normal, Insert (`i`/`a`/`o`/`O`), Visual (`v`/`V`), Command (`:`)\n\n\
## Editing\n\
`dd`/`yy`/`p`/`P`, `ciw`/`diw`/`yiw`, `x`/`r`, `.` (dot-repeat), `u`/`Ctrl-r`\n\n\
## Text objects\n\
`iw`/`aw`, `i\"`/`a\"`, `i(`/`a(`, `i{`/`a{`, `it`/`at`\n\n\
## Registers\n\
`\"ay` to yank into register a, `\"ap` to paste from it. `\"+` for system clipboard.\n\n\
## Ex commands\n\
`:w`, `:q`, `:wq`, `:e file`, `:set option`, `/search`\n\n\
## Macros\n\
`q{reg}` to record, `q` to stop, `@{reg}` to replay\n\n\
**Next:** [[tutorial:vim-differences|What's Different from Vim]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_VIM_DIFFERENCES: &str = "\
# What's Different from Vim\n\n\
## SPC leader instead of backslash\n\
MAE uses **Space** as the leader key (Doom Emacs style). This gives access \
to 14+ command groups:\n\
- `SPC f` — file operations\n\
- `SPC b` — buffer operations\n\
- `SPC w` — window operations\n\
- `SPC a` — AI commands\n\
- `SPC h` — MAE manual\n\
- `SPC p` — project commands\n\
...and more. A **which-key** popup appears after pressing SPC.\n\n\
## Scheme instead of VimL/Lua\n\
Configuration is in `init.scm` (R7RS Scheme), not `.vimrc` or `init.lua`.\n\
Edit it with `SPC f c`.\n\n\
```scheme\n\
(set-option! \"theme\" \"dracula\")\n\
(define-key \"normal\" \"g g\" \"goto-first-line\")\n\
(add-hook! \"buffer-open\" \"my-on-open\")\n\
```\n\n\
## Built-in AI\n\
- `SPC a p` — AI Chat (built-in conversation with editor context)\n\
- `SPC a a` — AI Agent (terminal-based, e.g. Claude Code)\n\
See [[tutorial:ai-setup|AI Setup]] for configuration.\n\n\
## No plugins — packages\n\
MAE uses a Scheme-based package system with `require-feature`/`provide-feature` \
instead of Vim plugins. See [[concept:package-system|Package System]].\n\n\
**Next:** [[tutorial:mae-navigation|MAE Navigation]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_WHAT_IS_MODAL: &str = "\
# What Is Modal Editing?\n\n\
In most editors, pressing `j` types the letter \"j\". In MAE, what a key does \
depends on which **mode** you're in.\n\n\
## Normal mode (default)\n\
Keys are **commands**: `j` moves down, `dd` deletes a line, `w` jumps to the next word.\n\
You navigate and manipulate text without ever reaching for the mouse.\n\n\
## Insert mode\n\
Keys type text, like a normal editor. Press `i` from Normal mode to enter Insert mode.\n\
Press `Escape` to return to Normal mode.\n\n\
## Visual mode\n\
Select text with movement keys. Press `v` from Normal mode.\n\n\
## Command mode\n\
Type commands after `:`. Press `:` from Normal mode.\n\n\
## Why modal?\n\
- Your fingers never leave the home row\n\
- Every key does something useful (no wasted Ctrl-Shift-Alt chords)\n\
- Composable: `d` + `w` = delete word, `c` + `i` + `\"` = change inside quotes\n\n\
**The golden rule:** If you get lost, press **Escape** to return to Normal mode.\n\n\
**Next:** [[tutorial:basic-movement|Basic Movement]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_BASIC_MOVEMENT: &str = "\
# Basic Movement\n\n\
All movement happens in **Normal mode** (press Escape if you're elsewhere).\n\n\
## Character movement\n\
```\n\
     k\n\
  h     l\n\
     j\n\
```\n\
`h` = left, `j` = down, `k` = up, `l` = right\n\n\
## Word movement\n\
- `w` — jump to next word start\n\
- `b` — jump to previous word start\n\
- `e` — jump to next word end\n\n\
## Line movement\n\
- `0` — beginning of line\n\
- `$` — end of line\n\
- `^` — first non-blank character\n\n\
## File movement\n\
- `gg` — go to first line\n\
- `G` — go to last line\n\
- `Ctrl-d` — half page down\n\
- `Ctrl-u` — half page up\n\n\
## Searching\n\
- `/pattern` — search forward\n\
- `n` — next match\n\
- `N` — previous match\n\n\
**Try it:** Open a file with `:e filename` and practice moving around!\n\n\
**Next:** [[tutorial:basic-editing|Basic Editing]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_BASIC_EDITING: &str = "\
# Basic Editing\n\n\
## Entering Insert mode\n\
- `i` — insert before cursor\n\
- `a` — insert after cursor\n\
- `o` — open new line below\n\
- `O` — open new line above\n\
- `Escape` — return to Normal mode\n\n\
## Deleting\n\
- `x` — delete character under cursor\n\
- `dd` — delete entire line\n\
- `dw` — delete from cursor to next word\n\
- `d$` — delete to end of line\n\n\
## Copy and paste\n\
- `yy` — yank (copy) entire line\n\
- `yw` — yank word\n\
- `p` — paste after cursor\n\
- `P` — paste before cursor\n\n\
## Undo and redo\n\
- `u` — undo\n\
- `Ctrl-r` — redo\n\n\
## Saving and quitting\n\
- `:w` — save\n\
- `:q` — quit\n\
- `:wq` — save and quit\n\
- `:q!` — quit without saving\n\n\
## The dot command\n\
`.` repeats your last edit. Delete a word with `dw`, then press `.` to delete another.\n\n\
**Next:** [[tutorial:mae-navigation|MAE Navigation]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_MAE_NAVIGATION: &str = "\
# MAE Navigation\n\n\
MAE's **SPC leader** gives fast access to every subsystem.\n\n\
## Files\n\
- `SPC f f` — fuzzy find file in project\n\
- `SPC f d` — file browser (directory listing)\n\
- `SPC f t` — toggle file tree sidebar\n\
- `SPC f r` — recent files\n\
- `SPC f c` — edit config (init.scm)\n\n\
## Buffers\n\
- `SPC b b` — switch buffer (fuzzy palette)\n\
- `SPC b d` — close current buffer\n\
- `SPC b n` / `SPC b p` — next / previous buffer\n\n\
## Windows\n\
- `SPC w v` — split vertically\n\
- `SPC w s` — split horizontally\n\
- `SPC w h/j/k/l` — focus left/down/up/right window\n\
- `SPC w q` — close window\n\
- `SPC w =` — balance window sizes\n\n\
## Project\n\
- `SPC p s` — search text in project (grep)\n\
- `SPC p f` — find file in project\n\n\
## Search\n\
- `/` — search in current buffer\n\
- `SPC p s` — search across project\n\
- `SPC SPC` — command palette (search for any command)\n\n\
## Help\n\
- `SPC h` — help menu\n\
- `SPC h s` — search help topics\n\
- `SPC h k` — describe key\n\
- `:help topic` — look up a topic\n\n\
**Next:** [[tutorial:mae-extending|Extending MAE]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_MAE_EXTENDING: &str = "\
# Extending MAE\n\n\
MAE is extensible via **R7RS Scheme** (the mae-scheme runtime).\n\n\
## The REPL\n\
- `:eval (+ 1 2)` — evaluate an expression (result shown in status bar)\n\
- `SPC e e` — evaluate current line\n\
- `SPC e b` — evaluate entire buffer\n\n\
## Configuration (init.scm)\n\
Your config file is `~/.config/mae/init.scm`. Open it with `SPC f c`.\n\n\
```scheme\n\
;; Set theme\n\
(set-option! \"theme\" \"dracula\")\n\n\
;; Custom keybinding\n\
(define-key \"normal\" \"g r\" \"lsp-find-references\")\n\n\
;; React to editor events\n\
(add-hook! \"buffer-save\" \"my-on-save\")\n\n\
;; Define a custom command\n\
(define-command \"hello\" \"Say hello\" \"my-hello-fn\")\n\
(define (my-hello-fn) (set-status \"Hello from Scheme!\"))\n\
```\n\n\
## Packages\n\
Place `.scm` files in `~/.config/mae/packages/`. Use `require-feature` and \
`provide-feature` to manage dependencies.\n\
See [[concept:package-system|Package System]] for details.\n\n\
## Full Scheme API\n\
MAE exposes ~50 functions and 18 variables to Scheme.\n\
See [[concept:scheme-api|Scheme API]] for the full reference, or use \
`:help scheme:function-name` for individual docs.\n\n\
See also: [[tutorial:ai-setup|Set up AI]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_AI_SETUP: &str = "\
# AI Setup\n\n\
MAE has two AI interfaces: **AI Chat** (built-in, `SPC a p`) and \
**AI Agent** (terminal, `SPC a a`). This guide walks you through \
configuring either one.\n\n\
## 1. Prerequisites\n\n\
You need a running MAE instance (you're reading this, so you have one) \
and an API key from at least one provider.\n\n\
## 2. Where config lives\n\n\
MAE's primary config surface is **`~/.config/mae/init.scm`** (Scheme): \
keybindings, `(mae!)` module selection, hooks, and every editor option via \
`(set-option!)` / `:set` / `:set-save` (the latter persists into init.scm). \
A narrow **`config.toml`** (a legacy bootstrap being retired) still holds the \
handful of startup-only settings the wizard needs: AI provider/model/key, \
theme/font, LSP paths, and daemon/collab connection. AI provider config is \
one of those legitimate config.toml settings, so that's where this guide \
puts it. Open config.toml with `SPC f P` (`:edit-settings`); open init.scm \
with `SPC f c` (`:edit-config`).\n\n\
If this is your first launch, the **setup wizard** runs automatically \
and handles provider selection, model, and API key.\n\n\
**Checkpoint:** `:describe-configuration` shows config file path and load status.\n\n\
## 3. Choose a provider\n\n\
### Claude (Anthropic)\n\
1. Get an API key: https://console.anthropic.com/settings/keys\n\
2. Add to `~/.config/mae/config.toml`:\n\
```toml\n\
[ai]\n\
provider = \"claude\"\n\
model = \"claude-sonnet-4-20250514\"\n\
api_key = \"sk-ant-...\"\n\
```\n\
3. Or use a secrets manager:\n\
```toml\n\
api_key_command = \"pass show anthropic/api-key\"\n\
```\n\n\
### OpenAI\n\
1. Get an API key: https://platform.openai.com/api-keys\n\
2. Config:\n\
```toml\n\
[ai]\n\
provider = \"openai\"\n\
model = \"gpt-4o\"\n\
api_key = \"sk-...\"\n\
```\n\n\
### Gemini (Google)\n\
1. Get an API key: https://aistudio.google.com/apikeys\n\
2. Config:\n\
```toml\n\
[ai]\n\
provider = \"gemini\"\n\
model = \"gemini-2.5-pro\"\n\
api_key = \"AI...\"\n\
```\n\n\
### DeepSeek\n\
1. Get an API key: https://platform.deepseek.com/api_keys\n\
2. Config:\n\
```toml\n\
[ai]\n\
provider = \"deepseek\"\n\
model = \"deepseek-chat\"\n\
api_key = \"sk-...\"\n\
```\n\n\
### Environment variable alternative\n\
Instead of putting keys in config, set one of:\n\
- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `DEEPSEEK_API_KEY`\n\
- Or the universal override: `MAE_AI_API_KEY`\n\n\
**Checkpoint:** `:ai-status` shows provider name, model, and \"configured\" status.\n\n\
## 4. Verify connectivity\n\n\
Run `:ai-ping` — you should see \"pong\" with a latency measurement.\n\n\
**Checkpoint:** `:ai-ping` returns success (not \"no provider configured\" or timeout).\n\n\
## 5. Test your agent\n\n\
Open AI chat with `SPC a p` (or `:ai-chat`) and try:\n\
> What file am I editing?\n\n\
The AI should respond with your current buffer name — this proves \
tool-calling is working end-to-end.\n\n\
**Checkpoint:** AI responds with current buffer name.\n\n\
## 6. Security hardening (recommended)\n\n\
Replace plaintext `api_key = \"...\"` with a command that fetches from \
your secrets manager:\n\
```toml\n\
# Linux (pass)\n\
api_key_command = \"pass show anthropic/api-key\"\n\n\
# macOS Keychain\n\
api_key_command = \"security find-generic-password -s mae-ai -w\"\n\
```\n\n\
**Checkpoint:** `:describe-configuration` shows `api_key_source: command` \
(not `plaintext`).\n\n\
## 7. AI Agent (terminal-based)\n\n\
`SPC a a` launches an external AI agent (Claude Code, gemini-cli, etc.) \
in MAE's embedded terminal. Configure which command to run:\n\
```toml\n\
[ai]\n\
editor = \"claude\"          # or \"gemini\", \"aider\", etc.\n\
```\n\n\
The agent gets access to MAE's editor tools via the MCP bridge.\n\n\
## Next steps\n\n\
- [[tutorial:ai-agent|AI Agent (Terminal)]] — deep dive on agent workflows\n\
- [[tutorial:ai-chat|AI Chat (Built-in)]] — conversation UI, persistence, budgets\n\
- [[tutorial:collab-setup|Collaborative Setup]] — share buffers across machines\n\
- [[concept:ai-as-peer|AI as Peer]] — architecture: how the AI is a peer actor\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_AI_AGENT: &str = "\
# AI Agent (Terminal)\n\n\
The **AI Agent** (`SPC a a`) runs an external tool like Claude Code or gemini-cli \
in MAE's embedded terminal.\n\n\
## How it works\n\
1. Press `SPC a a` — MAE opens a terminal and runs the configured `ai.editor` command\n\
2. The agent has access to your project files via the filesystem\n\
3. Via the MCP bridge, the agent can also call MAE's editor tools\n\n\
## Configuration\n\
```toml\n\
[ai]\n\
editor = \"claude\"          # or \"gemini\", or a custom command\n\
```\n\n\
## When to use\n\
- Autonomous coding tasks (write a feature, fix a bug)\n\
- Complex multi-file refactors\n\
- Tasks that need shell access (running tests, installing packages)\n\
- When you want the AI to drive and you review\n\n\
## Terminal controls\n\
- `Ctrl-\\ Ctrl-n` — exit terminal mode (return to Normal)\n\
- The agent's terminal is a full VT100 emulator (colors, scrollback)\n\n\
**Next:** [[tutorial:ai-chat|AI Chat (Built-in)]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_AI_CHAT: &str = "\
# AI Chat (Built-in)\n\n\
The **AI Chat** (`SPC a p`) is MAE's native conversation interface.\n\n\
## How it works\n\
1. Press `SPC a p` — the prompt line activates at the bottom of the conversation buffer\n\
2. Type your message and press Enter\n\
3. The AI responds with full editor context: it can see your buffers, LSP diagnostics, \
syntax trees, and debug state\n\n\
## What the AI can do\n\
- Read and edit buffers\n\
- Navigate files and project structure\n\
- Run registered commands\n\
- Query LSP (definitions, references, hover)\n\
- Inspect DAP debug state\n\
- Search the knowledge base\n\n\
## Configuration\n\
```toml\n\
[ai]\n\
provider = \"claude\"\n\
model = \"claude-sonnet-4-20250514\"\n\
permission = \"trusted\"      # readonly, standard, trusted, privileged\n\
budget_warn_tokens = 50000\n\
budget_limit_tokens = 100000\n\
```\n\n\
## Conversation persistence\n\
Conversations are saved per project in `.mae/conversation.json`.\n\
- `:ai-save` — manually save the conversation\n\
- `:ai-load` — load a saved conversation\n\
- `restore_session = true` — auto-restore on startup\n\n\
## Tips\n\
- Use `SPC a c` to cancel an in-flight AI request\n\
- Use `Escape` during AI operation to cancel and regain input\n\
- The token budget dashboard shows usage in the status bar\n\n\
See also: [[concept:ai-as-peer|AI as Peer]], [[concept:ai-modes|Agent vs Chat]]\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";

const TUTORIAL_COLLAB_SETUP: &str = "\
# Collaborative Setup\n\n\
This guide walks you through multi-user collaborative editing — sharing \
buffers and knowledge bases between MAE instances on different machines.\n\n\
## 1. Prerequisites\n\n\
- Two machines (or two MAE instances) on the same network\n\
- Both running MAE v0.11.1 or later\n\
- The `mae-daemon` binary (included in release artifacts)\n\n\
**Checkpoint:** `:verify` shows version >= 0.11.1.\n\n\
## 2. Start the daemon\n\n\
On the **host machine**, run:\n\
```bash\n\
mae-daemon --bind 0.0.0.0:9473\n\
```\n\n\
Note the host's IP address:\n\
- Linux: `hostname -I`\n\
- macOS: `ipconfig getifaddr en0`\n\n\
**Checkpoint:** Server logs \"listening on 0.0.0.0:9473\".\n\n\
## 3. Authenticate (trusted-peer `key` mode — recommended)\n\n\
MAE supports three auth modes — `none`, `psk`, and `key`. The recommended \
mode is **`key`**: each editor has an Ed25519 identity and the daemon trusts \
explicitly authorized peers over mTLS (no shared secret to leak). On **each \
client machine**, run:\n\
```bash\n\
mae setup-collab --server <host-ip>:9473\n\
```\n\
This generates the editor's identity and writes the `collab-auth-mode` / \
`collab-server-address` options into `init.scm` for you. (Pass `--ssh-key \
~/.ssh/id_ed25519` to reuse an existing SSH key.) Print a client's public \
identity with:\n\
```bash\n\
mae --collab-identity\n\
```\n\
Then authorize each client on the **host machine**:\n\
```bash\n\
mae-daemon authorize <pubkey-line>   # paste the line from --collab-identity\n\
```\n\n\
**Checkpoint:** `:describe-configuration` shows `collab_auth_mode: key`; \
`mae-daemon authorized` lists each client.\n\n\
> Secrets are never stored in plaintext config. If you must use the legacy \
> symmetric `psk` mode, set `collab-auth-mode` to `psk` and supply the key via \
> `(set-option! \"collab-psk-command\" \"pass show mae/collab-psk\")` (or `mae \
> setup-collab` in key mode and skip PSK entirely) — never a plaintext \
> `collab-psk` in config.toml.\n\n\
## 4. Connect from the second machine\n\n\
```\n\
:collab-connect <host-ip>:9473\n\
```\n\n\
`mae setup-collab` already set the address; to also connect automatically on \
startup, add to `init.scm`:\n\
```scheme\n\
(set-option! \"collab-server-address\" \"<host-ip>:9473\")\n\
(set-option! \"collab-auto-connect\" \"true\")\n\
```\n\
(or `:set collab-auto-connect true` then `:set-save` to persist).\n\n\
**Checkpoint:** `:collab-status` shows `connected`, peer count >= 1.\n\n\
## 5. Share a buffer\n\n\
On either machine, open a file and run `:collab-share`. \
On the other machine, `:collab-list` shows shared docs.\n\n\
**Checkpoint:** `:collab-status` shows shared doc count >= 1; \
edits appear on both sides.\n\n\
## 6. Share a knowledge base\n\n\
On the host: `:kb-share` — shares the active KB.\n\
On the peer: `:kb-join` — joins the shared KB.\n\n\
**Checkpoint:** `:collab-status` shows KB sync active; \
`:kb-list` shows shared nodes.\n\n\
## 7. Verify sync\n\n\
Edit a shared buffer on one machine — the change should appear on the \
other within 1 second. Create a KB node on one machine:\n\
```\n\
:kb-create \"test-sync\" \"Sync Test\" \"note\" \"Hello from machine A\"\n\
```\n\n\
**Checkpoint:** `:kb-get \"test-sync\"` on the other machine returns the node.\n\n\
## 8. Troubleshooting\n\n\
Run `:collab-doctor` for comprehensive diagnostics. Common issues:\n\
- **\"connection refused\"** — firewall blocking port 9473, or server not running\n\
- **\"auth failed\"** — in `key` mode the client isn't authorized yet (run \
`mae-daemon authorize`); in `psk` mode the keys don't match\n\
- **\"timeout\"** — wrong IP address, or machines not on same network\n\n\
Open firewall if needed:\n\
- macOS: allow in System Settings > Network > Firewall\n\
- Linux: `sudo firewall-cmd --add-port=9473/tcp --permanent`\n\n\
**Checkpoint:** `:collab-doctor` returns all checks green.\n\n\
## Next steps\n\n\
- [[lesson:collab-setup|Collaborative Editing (detailed lesson)]] — systemd, client-frame, network setup\n\
- [[concept:collab-architecture|Architecture]] — how sync works under the hood\n\
- [[concept:kb-sharing|KB Sharing]] — collaborative knowledge bases in depth\n\
- [[tutorial:ai-setup|AI Setup]] — configure AI providers\n\n\
* Getting Help\n\
- `SPC h` opens the MAE manual\n\
- `SPC h s` searches all help topics\n\
- `:help TOPIC` looks up any command, option, or concept\n\
- `SPC h k` describes what a key does\n\
- `SPC SPC` opens the command palette — search for anything\n";
