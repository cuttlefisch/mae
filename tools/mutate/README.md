# mae-mutate

Declared, re-runnable mutation checks. A test that guards a mechanism must be
shown to fail without that mechanism; the mutants that show it are committed
next to the code, in `mutants/<area>.toml`, and this tool applies them one at a
time and judges each.

It does not invent mutants (cargo-mutants does that, and cannot express "this
exact guard, negated, must fail this exact test"). Every mutant is written by
the author of the guard it breaks.

## Format

Runners are defined once, in `mutants/runners.toml`:

```toml
[runner.daemon]
workspace = "daemon"                     # cwd, relative to the repository root
build = ["cargo", "nextest", "run", "--no-run", "{targets}", "-E", "{filter}"]
test  = ["cargo", "nextest", "run", "{targets}", "-E", "{filter}"]
targets = []                             # default; a mutant may override
caught = ["FAIL [", "TIMEOUT ["]         # any of these in the output: caught
count = ["{n} tests run", "{n} test run"]  # summed; the control needs > 0
queue_class = "rust-enlace"              # optional, default "rust"
# optional: unviable (compile-error markers), rebuilt (default "Compiling "),
# retry_on_kill (default 1)
```

`{targets}` is replaced by zero or more arguments; `{filter}` is substituted
wherever it appears, and `test` must use it.

Every marker (`caught`, `count`, `unviable`, `rebuilt`) is matched against the
command's output with ANSI escape sequences removed, so `CARGO_TERM_COLOR=always`
(set in CI) does not hide them. The log keeps the output as written.

Each area file holds its mutants:

```toml
[[mutant]]
id = "ws-outbound/no-base64-factor"      # unique and stable: the --only key
runner = "daemon"
filter = "test(outbound)"                # the tests that must fail
targets = ["--lib"]                      # optional
ref = "ADR-099 A2"                       # optional
equivalent = ""                          # non-empty = why it cannot be caught; listed, not run
[[mutant.edit]]                          # one or more, in order; several files allowed
file = "daemon/src/ws/mod.rs"
find = '''let base64 = max_document_bytes.div_ceil(3).saturating_mul(4);'''
replace = '''let base64 = max_document_bytes;'''
```

Each `find` must occur exactly once in the file, as the mutant's earlier edits
leave it. Name a mutant for what the mutation does, neutrally.

## Commands

```sh
make mutate-check                         # anchors, ids, runners; runs no cargo
make mutate ARGS="--since origin/main"    # the mutants touched by this branch
make mutate ARGS="--area hub-verify"      # one area
make mutate ARGS="--only a/x,a/y"         # these ids, always re-run
make mutate ARGS="--shard 2/4"            # a deterministic quarter, by id
```

`mae-mutate list [selection]` prints the selected ids without running anything.
`--since REF` selects the mutants whose edited files changed in
`REF...HEAD` or in the working tree, and every mutant of a changed list file
(all of them, if `runners.toml` changed).

A run:

1. restores any file a killed run left mutated (from the journal);
2. refuses to start if a file a mutant edits has uncommitted changes;
3. runs each distinct (runner, targets, filter) on the healthy code: it must
   pass and run at least one test, or the run stops;
4. for each mutant, journals the originals, applies the edits, builds (timed),
   then tests (timed), and restores. A build that recompiled nothing is
   rebuilt once after its files' mtimes are bumped, and never judged on a stale
   binary;
5. prints and records each verdict as it is known.

Verdicts: `caught`, `SURVIVED`, `unviable` (did not compile), `no-result`
(killed with no marker, after one retry). The summary is
`N of N caught; survivors: [...]`, then the other classes, the build and test
time totals, and the slowest mutants. The exit code is non-zero on any
survivor, unviable, no-result or failed control.

State lives in the git directory (`mae-mutate/`: journal, lock, results,
logs), never in the tree. A rerun skips ids already recorded on the same code
(HEAD, working-tree diff and untracked files outside `mutants/`, plus the
mutant's and runner's own definitions); a verdict recorded on other code runs
again. `--fresh` forgets every result; `--only` always re-runs what it names.

## The queue

Every build and test runs under a prefix. By default it is
`trabajo <queue_class> mae:mutants:<area> --` when `trabajo` is on `PATH`, and
nothing otherwise. `MUTATE_QUEUE=off` disables it (CI does); `MUTATE_PREFIX`
replaces it, with `{class}` and `{area}` filled in, e.g.
`MUTATE_PREFIX="systemd-run --user --scope -q -p MemoryMax=6G env CARGO_BUILD_JOBS=2"`.

Do not run the tool itself inside a queued job. `trabajo` asks Slurm for a job
step, and inside a job that step is placed in the enclosing job, whose CPUs and
memory the tool already holds, so it waits until the job's time limit with no
output. A `run` inside a Slurm job (`SLURM_JOB_ID` set) with the default prefix
is refused before anything runs; run the tool outside the queue (it queues its
own steps), or set `MUTATE_QUEUE=off` to run every step in the enclosing job.

## Hung tests

A mutant can make a guarding test hang rather than fail. Every runner's test
command passes nextest's `--profile mutate`, defined in each workspace's
`.config/nextest.toml` with `slow-timeout = { period = "60s", terminate-after =
5 }`: nextest counts from when the test starts (time waiting in the queue does
not count), kills the test's process group after five minutes, and reports
`TIMEOUT [`, which the runners read as caught. A control run that times out is
a failed control.

## Cadence

- Each change that adds a guard adds its mutants to the area's list, in the
  same change.
- Between review rounds: `--since` the base branch.
- Once before merge: the full list, locally, or by adding the `mutation`
  label to the pull request.
- CI: `--check` on every pull request; every list, sharded, nightly
  (`.github/workflows/mutation.yml`).

The tool's own guards are in `mutants/tools-mutate.toml`, caught by its tests
(`make mutate-test`).
