# rst agent guide

Operational handbook for running `rst` (the Rustickers CLI) from an agent. Every fenced
command line below is parser-tested against the real argument definitions; lines that begin
with `#` are commentary.

## Mental model

- `rst` writes stickers to a local SQLite store. It **creates and inspects**; the desktop app
  **executes**. `rst command ...` saves a sticker — it does not run the program. The sticker
  runs when its window opens, on its schedule, or when the app starts while it is armed.
- `rst result <id>` prints the latest stored output of a command sticker; it never executes or
  waits.
- Every call replies with a human report, or — with `--json` — exactly one JSON object that
  always carries an `ok` field. Parse the payload; do not scrape prose.

## Global flags (outer level only)

- `--json` one JSON object per call, errors included (except text help/version). It is incompatible
  with `text exec`, whose child owns stdio. Grammar errors exit 2; validation/runtime failures exit 1;
  successful commands exit 0. Child statuses pass through, cancellation is 130, launch failures 126/127.
- `--data-dir PATH` use `PATH/stickers.db` instead of the user's store **and disable desktop
  IPC**. This is the isolation switch: every experiment, test or dry run an agent performs
  must carry it.
- `--offline` save without notifying the running desktop app. Delivery reports read
  `sent`, `unavailable`, `offline`, `not_requested`.

## Grammar

- `rst list` — all stickers, open and closed.
- `rst show ID [--content-only]` — everything stored about one sticker.
- `rst result ID` — latest stored command output; never executes.
- `rst open ID` / `rst close ID` — request a window; schedules keep running.
- `rst delete ID --yes` — permanent delete.
- `rst view PATH` — transient preview (desktop required).
- `rst note [TEXT | --file PATH | --file -]` — markdown note; TEXT is positional.
- `rst command [OPTIONS] -- PROGRAM ARGS` — save a direct-argv command sticker.
- `rst command [OPTIONS] --shell SCRIPT` — same, but SCRIPT runs in a shell (exclusive).
- `rst text list [--source saved|clipboard|all] [--state all|open|closed] [--json]` —
  text/clipboard metadata only.
- `rst text exec [--source ...] [--state ...] [--id sticker:N|clipboard:N]
  [--non-interactive] -- PROGRAM ARGS` — hand one stored text to a child program.
- `rst skill list` / `rst skill show NAME` / `rst skill run NAME [--set KEY=VALUE ...]
  [--dry-run]` / `rst skill guide` — the recipe catalogue.

## Commands do not run through a shell

With `-- PROGRAM ARGS`, argv is preserved exactly: empty arguments stay empty, quotes are
data, and the first token is resolved on `PATH` and executed directly. Pipes, `>`, `&&` and
`$(...)` are literal text here. When you genuinely want a shell, pass the whole script as the
one `--shell SCRIPT` value: on Windows that is PowerShell (`pwsh`, falling back to
`powershell`); elsewhere it is `sh -c`. The script is code you chose — never interpolate
untrusted text into it.

```text
# a direct command, run in this checkout
rst command --dir . -- cargo test
# pipes and redirects need the explicit shell form
rst command --shell "git status | head -20"
# a selection sticker whose prompt is one argv segment
rst command --accept-selection --result markdown -- copilot -p "Summarize this: {{RUSTICKERS_SELECTION}}"
```

## Selection contract

`--accept-selection` offers the sticker on the selected-text hotkey.

- `{{RUSTICKERS_SELECTION}}` is substituted into **argument positions only**, as one argv
  value, as data — it is never re-parsed, never expanded again, and rejected in the program
  position. Keep the placeholder inside the one argument that should receive the text.
- The selection is always also exported as the `RUSTICKERS_SELECTION` environment variable.
  Prefer the variable inside static shell code, quoted as data (for example, `"$RUSTICKERS_SELECTION"`
  in POSIX shell). Never evaluate the value as code or splice it into the script string.
- Passing text to an interpreter's code argument (`-c`, `-Command`) can execute it. The
  built-in skills keep code static and pass data via argv or `--env`.

## Text workflows

`rst text list` prints metadata and bounded plaintext previews; locked entries expose only public titles. `rst text exec` substitutes the chosen
text into argv: `{{txt}}` is replaced as data in each argument that carries it, and when no
argument has the placeholder the text is appended as one final argument. Child stdio and exit
status pass through. Agents use `--id` with `--non-interactive` to skip the chooser.

```text
# metadata only, machine-readable
rst text list --json
# hand one capture to ripgrep without ever opening a chooser
rst text exec --id clipboard:42 --non-interactive -- rg "{{txt}}"
```

## Schedules

`--cron` takes a 6-field Quartz expression (second first): `0 */5 * * * *` is every five
minutes; the five-field crontab form is rejected at the door. A scheduled sticker keeps
ticking while the app runs, whether or not its window is open.

## Skills

- `rst skill list` / `rst skill show <name>` are pure discovery: no database, no PATH
  lookups. `show` parser-validates its expansion; when a variable has no default it prints
  the template marked **not runnable** plus the `--set` form — never a fabricated command.
- `rst skill run <name> [--set KEY=VALUE ...] [--dry-run]` creates the sticker. `--set`
  rejects unknown keys, duplicate keys and empty names. `${name}` recipe substitution is one-pass:
  inserted values are not rescanned for more recipe variables. The desktop selection placeholder
  `{{RUSTICKERS_SELECTION}}` still has its separate runtime meaning in selection arguments.
- `--dry-run` runs the full semantic validation (program on `PATH`, selection contract,
  cron) and writes nothing. Use it before creating, and always with `--data-dir`.
- In JSON replies the `argv` array is canonical; `command_line` is a convenience line quoted
  for the host shell (PowerShell rules on Windows, POSIX single quotes elsewhere). Drive
  programs from `argv`, not from quoted text.
- `rst skill guide` prints this guide from inside the binary — no database needed.

## The catalogue

| Skill | What it creates | Platform |
| --- | --- | --- |
| ask | selection → one prompt argument → markdown reply | any tool on PATH |
| translate | one prompt argument holding the instruction, `--set language=...` | any tool on PATH |
| explain-code | one prompt argument explaining the selected code | any tool on PATH |
| web-search | static PowerShell code, `--env SEARCH_ENGINE=...` data only | Windows |
| format-json | static PowerShell code, `--env JSON_DEPTH=...` data only | Windows |
| watch | scheduled `--shell` script, `--stream` output | platform shell |
| html-report | `--shell` script printing an HTML document | platform shell |
| svg-chart | `--shell` script printing an SVG document | platform shell |
| note | markdown note pinned to the desktop | any |

```text
# discover the catalogue and one recipe in detail
rst skill list
rst skill show translate
# create a Japanese menu entry after checking it validates
rst skill run translate --set language=Japanese --dry-run
# re-run the tests every five minutes
rst skill run watch --set script="cargo test" --set cron="0 */5 * * * *" --dry-run
# the plain ask sticker with its default tool
rst skill run ask --dry-run
# this handbook, from inside the binary
rst skill guide
# a pinned note
rst note "Remember this"
# everything above, safely, inside an isolated store
rst --data-dir ./demo-db skill list
```
