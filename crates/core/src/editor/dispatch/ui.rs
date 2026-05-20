// @ai-caution: [dispatch] Remaining UI commands after split into
// dispatch/{help,terminal,project,kb,config}.rs. Dashboard, AI, palette,
// describe, link editing, demos, and misc UI commands live here.

//! UI commands: palette, AI, describe, link editing, demos, misc.

use crate::buffer::Buffer;
use crate::command_palette::CommandPalette;
use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch UI, AI, describe, link editing, demo, and misc commands.
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_ui(&mut self, name: &str) -> Option<bool> {
        match name {
            "view-messages" => {
                self.open_messages_buffer();
            }
            "dashboard" => {
                let idx = if let Some(idx) = self
                    .buffers
                    .iter()
                    .position(|b| b.kind == crate::BufferKind::Dashboard)
                {
                    idx
                } else {
                    self.buffers.push(Buffer::new_dashboard());
                    self.buffers.len() - 1
                };
                let prev = self.active_buffer_idx();
                self.alternate_buffer_idx = Some(prev);
                self.display_buffer(idx);
                self.set_mode(Mode::Normal);
            }
            "toggle-scratch-buffer" => {
                let current = self.active_buffer_idx();
                let is_scratch = self.buffers[current].kind == crate::BufferKind::Text
                    && self.buffers[current].name == "[scratch]";
                if is_scratch {
                    let alt = self.alternate_buffer_idx.unwrap_or(0);
                    if alt < self.buffers.len() && alt != current {
                        self.alternate_buffer_idx = Some(current);
                        self.display_buffer(alt);
                        self.sync_mode_to_buffer();
                    }
                } else {
                    let idx =
                        if let Some(idx) = self.buffers.iter().position(|b| {
                            b.kind == crate::BufferKind::Text && b.name == "[scratch]"
                        }) {
                            idx
                        } else {
                            self.buffers.push(Buffer::new());
                            self.buffers.len() - 1
                        };
                    self.alternate_buffer_idx = Some(current);
                    self.display_buffer(idx);
                    self.set_mode(Mode::Normal);
                }
            }

            "show-buffer-keys" => {
                self.buffer_keys_popup = true;
            }

            "file-info" => {
                let idx = self.active_buffer_idx();
                let buf = &self.buffers[idx];
                let total = buf.line_count();
                let row = self.window_mgr.focused_window().cursor_row + 1;
                let pct = (row * 100).checked_div(total).unwrap_or(0);
                let name = buf
                    .file_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| buf.name.clone());
                let modified = if buf.modified { " [+]" } else { "" };
                self.set_status(format!(
                    "\"{}\"{}  line {} of {} --{}%--",
                    name, modified, row, total, pct
                ));
            }

            // Link following (gx / Enter on links in any buffer)
            "open-link-at-cursor" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let col = win.cursor_col;
                let buf = &self.buffers[idx];

                // Check display regions first (link concealment in text buffers).
                if !buf.display_regions.is_empty() {
                    let line_chars: Vec<char> = buf
                        .rope()
                        .line(row)
                        .chars()
                        .filter(|c| *c != '\n' && *c != '\r')
                        .collect();
                    let line_byte_start = buf.rope().char_to_byte(buf.rope().line_to_char(row));
                    // The cursor col is a rope col — find the matching display region.
                    let cursor_byte = line_byte_start + {
                        let line_str: String = line_chars.iter().collect();
                        line_str
                            .char_indices()
                            .nth(col)
                            .map(|(b, _)| b)
                            .unwrap_or(line_str.len())
                    };
                    if let Some(region) = buf
                        .display_regions
                        .iter()
                        .find(|r| cursor_byte >= r.byte_start && cursor_byte < r.byte_end)
                    {
                        if let Some(ref target) = region.link_target {
                            let target = target.clone();
                            self.handle_link_click(&target);
                            return Some(true);
                        }
                    }
                }

                // Check conversation rendered links first (from markdown stripping)
                if let Some(conv) = buf.conversation() {
                    if let Some(link) = conv.link_at_position(row, col) {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return Some(true);
                    }
                }

                // Then check buffer link_spans (populated by renderer for conversation/shell)
                if !buf.link_spans.is_empty() {
                    let line_start_byte = buf.rope().char_to_byte(buf.rope().line_to_char(row));
                    let click_byte = line_start_byte + col;
                    if let Some(link) = buf
                        .link_spans
                        .iter()
                        .find(|s| click_byte >= s.byte_start && click_byte < s.byte_end)
                    {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return Some(true);
                    }
                }

                // Fall back: detect links in current line, find one containing cursor col
                let line_text: String = buf.rope().line(row).chars().collect();
                let links = crate::link_detect::detect_links(&line_text);
                for link in &links {
                    let link_char_start = line_text[..link.byte_start].chars().count();
                    let link_char_end = line_text[..link.byte_end].chars().count();
                    if col >= link_char_start && col < link_char_end {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return Some(true);
                    }
                }
                self.set_status("No link under cursor");
            }

            "command-palette" => {
                self.command_palette = Some(CommandPalette::from_registry(&self.commands));
                self.set_mode(Mode::CommandPalette);
            }

            // AI
            "ai-prompt" | "ai-chat" => {
                self.open_conversation_buffer();
                // If AI is not configured, show setup guidance in the output buffer
                // and stay in Normal mode so the user can read/copy the URLs.
                if !self.ai_configured {
                    if let Some(ref pair) = self.conversation_pair {
                        let out_idx = pair.output_buffer_idx;
                        if out_idx < self.buffers.len() {
                            let guidance = "\
AI is not configured yet.

Quick setup:
  1. Get an API key from your provider:
     - Claude:   https://console.anthropic.com/settings/keys
     - OpenAI:   https://platform.openai.com/api-keys
     - Gemini:   https://aistudio.google.com/apikey
     - DeepSeek: https://platform.deepseek.com/api_keys

  2. Set the environment variable:
     export ANTHROPIC_API_KEY=sk-ant-...

  3. Or run: mae --init-config
     Then edit ~/.config/mae/config.toml

  4. Restart MAE.

For full setup guide: :help ai-setup";
                            self.buffers[out_idx].replace_contents(guidance);
                        }
                    }
                    self.set_status("AI not configured \u{2014} :help ai-setup for setup guide");
                }
            }
            "ai-set-mode" => {
                let modes = vec!["standard", "plan", "auto-accept"];
                self.command_palette = Some(CommandPalette::for_ai_mode(&modes));
                self.set_mode(Mode::CommandPalette);
            }
            "ai-set-profile" => {
                let profiles = vec!["pair-programmer", "explorer", "planner", "reviewer"];
                self.command_palette = Some(CommandPalette::for_ai_profile(&profiles));
                self.set_mode(Mode::CommandPalette);
            }
            "ai-cancel" => {
                let status = match self.conversation_mut() {
                    Some(conv) if conv.streaming => {
                        conv.end_streaming();
                        conv.push_system("[cancelled]");
                        "[AI] Cancelled"
                    }
                    Some(_) => "No active AI request to cancel",
                    None => "No AI conversation active",
                };
                self.set_status(status);
                self.ai_cancel_requested = true;
            }

            // Describe
            "describe-key" => {
                self.awaiting_key_description = true;
                self.set_status("Describe key: press a key sequence (Esc to cancel)");
            }
            "describe-command" => {
                self.command_palette = Some(CommandPalette::for_describe(&self.commands));
                self.set_mode(Mode::CommandPalette);
            }
            "describe-option" => {
                self.show_all_options();
            }
            "describe-configuration" => {
                self.show_configuration_report();
            }
            "describe-bindings" => {
                self.show_bindings_report();
            }
            "describe-module" => {
                let arg = self.command_line.trim().to_string();
                let module_name = if arg.is_empty() { None } else { Some(arg) };
                self.show_module_report(module_name.as_deref());
            }
            "describe-module-at-cursor" => {
                // Extract module name from current line (first whitespace-delimited word).
                let line_text = {
                    let idx = self.active_buffer_idx();
                    let buf = &self.buffers[idx];
                    let row = self.window_mgr.focused_window().cursor_row;
                    if row < buf.line_count() {
                        buf.line_text(row)
                    } else {
                        String::new()
                    }
                };
                let name = line_text
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty()
                    && !name.starts_with('-')
                    && !name.starts_with('=')
                    && name != "Module"
                    && name != "Total:"
                    && name != "Press"
                {
                    self.show_module_report(Some(&name));
                }
            }
            "describe-mode" => {
                self.show_mode_report();
            }

            // AI agent launcher
            "open-ai-agent" => {
                // Prefer git root so agents operate at the repository level,
                // not a subcrate Cargo.toml directory.
                let agent_cwd = self.git_or_project_root();
                let shell_name = format!("*AI:{}*", self.ai_editor);
                let mut buf = Buffer::new_shell(shell_name);
                buf.agent_shell = true;
                self.buffers.push(buf);
                let new_idx = self.buffers.len() - 1;
                if let Some(cwd) = agent_cwd {
                    self.shell.cwds.insert(new_idx, cwd);
                }
                // @ai-caution: [window-split] Agent shells MUST use
                // switch_to_buffer_non_conversation() + split_root(), NOT
                // display_buffer_and_focus(). The latter steals conversation
                // windows. Fixed in commit 8a52851.
                self.switch_to_buffer_non_conversation(new_idx);
                // Focus the window showing the agent shell.
                let agent_win_id = self
                    .window_mgr
                    .iter_windows()
                    .find(|w| w.buffer_idx == new_idx)
                    .map(|w| w.id);
                if let Some(wid) = agent_win_id {
                    self.window_mgr.set_focused(wid);
                }
                let cmd = self.ai_editor.clone();
                self.shell.agent_spawns.push((new_idx, cmd));
                self.set_mode(Mode::ShellInsert);
            }

            // Demo buffers
            "open-demo-tables" => {
                self.open_demo("Tables", DEMO_TABLES);
            }
            "open-demo-markup" => {
                self.open_demo("Markup", DEMO_MARKUP);
            }
            "open-demo-agenda" => {
                self.open_demo("Agenda", DEMO_AGENDA);
            }

            // Edit a link under cursor: open a mini-dialog with URL + Label fields.
            "edit-link" => {
                use crate::command_palette::{
                    MiniDialogContext, MiniDialogField, MiniDialogKind, MiniDialogState,
                    PalettePurpose,
                };
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let col = win.cursor_col;
                let buf = &self.buffers[idx];

                // Compute cursor byte offset
                let line_byte_start = buf.rope().char_to_byte(buf.rope().line_to_char(row));
                let line_chars: Vec<char> = buf
                    .rope()
                    .line(row)
                    .chars()
                    .filter(|c| *c != '\n' && *c != '\r')
                    .collect();
                let line_str: String = line_chars.iter().collect();
                let cursor_byte = line_byte_start
                    + line_str
                        .char_indices()
                        .nth(col)
                        .map(|(b, _)| b)
                        .unwrap_or(line_str.len());

                // Find link region at cursor, or the next link region
                let region = buf
                    .display_regions
                    .iter()
                    .find(|r| {
                        r.link_target.is_some()
                            && cursor_byte >= r.byte_start
                            && cursor_byte < r.byte_end
                    })
                    .or_else(|| {
                        crate::display_region::next_link_region(&buf.display_regions, cursor_byte)
                            .and_then(|(s, _)| {
                                buf.display_regions
                                    .iter()
                                    .find(|r| r.link_target.is_some() && r.byte_start == s)
                            })
                    });

                if let Some(region) = region {
                    // Extract raw link text from the buffer
                    let raw_text: String = buf
                        .rope()
                        .byte_slice(region.byte_start..region.byte_end)
                        .chars()
                        .collect();
                    let is_org = buf
                        .file_path()
                        .and_then(|p| p.extension())
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("org"))
                        .unwrap_or(false);

                    let (url, label) = if is_org {
                        crate::display_region::parse_org_link(&raw_text)
                            .map(|(u, l)| (u, l.unwrap_or_default()))
                            .unwrap_or_else(|| (raw_text.clone(), String::new()))
                    } else {
                        crate::display_region::parse_md_link(&raw_text)
                            .unwrap_or_else(|| (raw_text.clone(), String::new()))
                    };

                    let state = MiniDialogState {
                        kind: MiniDialogKind::EditLink,
                        fields: vec![
                            MiniDialogField {
                                label: "URL".to_string(),
                                value: url,
                                placeholder: "https://...".to_string(),
                            },
                            MiniDialogField {
                                label: "Label".to_string(),
                                value: label,
                                placeholder: "Link text".to_string(),
                            },
                        ],
                        active_field: 0,
                        context: MiniDialogContext::LinkEdit {
                            buf_idx: idx,
                            byte_start: region.byte_start,
                            byte_end: region.byte_end,
                            is_org,
                        },
                    };
                    self.mini_dialog = Some(state);
                    // Open an empty palette in MiniDialog mode — renderers check mini_dialog
                    self.command_palette = Some(crate::command_palette::CommandPalette {
                        query: String::new(),
                        entries: Vec::new(),
                        filtered: Vec::new(),
                        selected: 0,
                        purpose: PalettePurpose::MiniDialog,
                        query_selected: false,
                    });
                    self.set_mode(Mode::CommandPalette);
                    self.set_status("Edit link — Tab: next field, Enter: apply, Esc: cancel");
                } else {
                    self.set_status("No link at cursor");
                }
            }

            _ => return None,
        }
        self.mark_full_redraw();
        Some(true)
    }

    fn open_demo(&mut self, label: &str, content: &str) {
        let name = format!("*Demo: {}*", label);
        let buf_idx = if let Some(idx) = self.find_buffer_by_name(&name) {
            idx
        } else {
            let mut buf = Buffer::new();
            buf.name = name;
            buf.kind = crate::BufferKind::Demo;
            buf.read_only = false;
            self.buffers.push(buf);
            let idx = self.buffers.len() - 1;
            self.buffers[idx].insert_text_at(0, content);
            self.buffers[idx].modified = false;
            idx
        };
        self.display_buffer_and_focus(buf_idx);
    }
}

const DEMO_TABLES: &str = "\
* Demo: Tables
  This is an interactive demo. Edit freely — changes won't be saved.
  Press q to close.

** Org Table
| Name    | Age | City       |
|---------+-----+------------|
| Alice   |  30 | New York   |
| Bob     |  25 | London     |
| Charlie |  35 | Tokyo      |

  Try: Tab to move between cells, S-Tab to go back.
  Try: SPC m b a to align columns after editing.

** Markdown Table
| Language | Typing     | GC   |
|----------|------------|------|
| Rust     | Static     | None |
| Go       | Static     | Yes  |
| Python   | Dynamic    | Yes  |
";

const DEMO_MARKUP: &str = "\
* Demo: Markup
  This is an interactive demo. Edit freely — changes won't be saved.

** Text Formatting
  *bold text* and /italic text/ and =verbatim= and ~code~
  +strikethrough text+

** Blockquotes
> This is a blockquote.
> It can span multiple lines.
>> Nested blockquotes work too.

** Horizontal Rules
-----

** Headings with TODO and Priority
*** TODO [#A] Urgent task                                      :work:urgent:
*** DONE [#C] Completed task                                   :personal:

** Lists
- Unordered item 1
- Unordered item 2
  - Nested item
- [ ] Checkbox unchecked
- [x] Checkbox checked

1. Ordered item 1
2. Ordered item 2

** Links
  See [[concept:buffer]] for buffer docs.
  External: https://github.com/cuttlefisch/mae
";

const DEMO_AGENDA: &str = "\
* Demo: Agenda & TODO
  This is an interactive demo. Edit freely — changes won't be saved.
  Run :agenda to see these items in the agenda view.

** TODO [#A] Fix critical bug in parser                        :bug:urgent:
** TODO [#B] Write unit tests for table module                 :testing:
** NEXT [#B] Review pull request from contributor              :review:
** WAIT Waiting on upstream API change                         :blocked:
** DONE [#C] Update documentation for v0.7                     :docs:
** TODO Implement smart list continuation                      :feature:
";
