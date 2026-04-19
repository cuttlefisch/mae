//! Vimtutor-style interactive tutorial accessible via `:tutor` or `SPC h t`.

/// Tutorial content covering MAE basics.
pub const TUTORIAL_CONTENT: &str = r#"
===============================================================================
=    W e l c o m e   t o   t h e   M A E   T u t o r i a l                   =
===============================================================================

MAE (Modern AI Editor) is an AI-native Lisp machine editor. This tutorial
covers the essentials. The buffer is editable — try the commands as you read!

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 1: NAVIGATION
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  h - move left       j - move down       k - move up       l - move right

  Try moving around this text using h, j, k, l.

  w - next word start          b - previous word start
  e - next word end            0 - line start
  $ - line end                 gg - first line
  G - last line                Ctrl-d - half page down
  Ctrl-u - half page up

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 2: MODES
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  MAE uses vi-style modal editing:

  Normal mode (default) — navigation and commands
  Insert mode          — type text
  Visual mode          — select text
  Command mode         — ex commands (: prefix)

  Press i to enter Insert mode. Press Escape to return to Normal mode.
  Press v to enter Visual mode. Press : to enter Command mode.

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 3: EDITING
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  i - insert before cursor     a - insert after cursor
  o - open line below          O - open line above
  x - delete character         dd - delete line
  yy - yank (copy) line        p - paste after
  u - undo                     Ctrl-r - redo
  . - repeat last edit

  Try deleting this line with dd, then undo with u.

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 4: FILES & BUFFERS
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  :w          - save file
  :e <file>   - open file
  :q          - quit (fails if unsaved changes)
  :wq or :x   - save and quit
  SPC f f     - find file (fuzzy picker)
  SPC f d     - file browser
  SPC b b     - switch buffer (palette)
  SPC b k     - close buffer

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 5: AI FEATURES
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  MAE treats the AI agent as a peer — it calls the same primitives as you.

  SPC a p     - AI prompt (send a message to the AI)
  SPC a i     - open AI conversation buffer
  SPC a a     - launch AI agent in a shell (e.g. Claude Code)

  The AI can read/edit buffers, navigate, search, and use LSP/DAP tools.

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 6: SCHEME REPL
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  MAE is extensible via R7RS Scheme (Steel).

  SPC e e     - evaluate current line
  SPC e b     - evaluate entire buffer
  :eval <expr> - evaluate a Scheme expression

  Try: :eval (+ 1 2)

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 7: LSP & DIAGNOSTICS
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  gd          - go to definition
  gr          - find references
  K           - hover documentation
  SPC l d     - show diagnostics

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 8: TERMINAL SHELL
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  SPC t t     - open terminal
  Ctrl-\ Ctrl-n - exit terminal to Normal mode
  SPC e s     - send line to terminal
  SPC e S     - send selection to terminal

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 9: HELP SYSTEM
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  SPC h h     - open help
  SPC h k     - describe key (press a key to see what it does)
  SPC h c     - describe command
  SPC h o     - describe option
  :help <topic> - open help for a topic

~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
                         Lesson 10: LEADER KEY SEQUENCES
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

  SPC is the leader key. Press SPC and wait to see available actions.

  SPC f ...   - file operations
  SPC b ...   - buffer operations
  SPC w ...   - window operations
  SPC a ...   - AI operations
  SPC h ...   - help
  SPC t ...   - toggles/themes
  SPC l ...   - LSP operations
  SPC q ...   - quit

===============================================================================
  That concludes the MAE tutorial. Happy editing!

  For more: SPC h h (help), or read the README.
===============================================================================
"#;
