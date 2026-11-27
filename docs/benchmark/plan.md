# Agent benchmark: plan

Status: draft, not yet run. No results exist yet. Any number in this file is an example of a table's
shape, not a measurement.

## The question

Do coding agents finish everyday pull request work on a Forgejo repository more reliably when they
can run `gh` through the shim than when they have only `fj`, Forgejo's own CLI?

Agents already know `gh` well. They have seen its flags in a great many repositories and scripts.
`fj` is younger and its commands differ. Without the shim an agent has to discover them, and in
practice it often guesses a flag, fails, reads `--help` and tries again, or gives up and calls the
API with `curl`. If a benchmark confirms this, the README should say so near the top, because it
gives a reason to install the shim even for someone who never opens a GUI app.

The benchmark must be fair to `fj`. A table that beats a handicapped baseline convinces nobody, so
the main comparison gives each tool the setup its own docs recommend, and the harness, prompts and
raw logs ship in this repository so anyone can rerun them.

## What it compares

**Agents.** Claude Code and Codex CLI, run headless (no UI, a machine-readable log of every step):
`claude -p --output-format stream-json` and `codex exec --json`. Each run records the agent's
version and model. Both use the model their CLI picks by default on the day of the run, so the table
shows what a user gets without changing settings.

**Arms.** An arm is one setup under test. Every arm runs in the same container image against the
same seeded Forgejo, with the same task prompt. Only the installed tool and one instruction file
differ.

| Arm | Tool on `PATH` | `AGENTS.md` and `CLAUDE.md` | Role |
|---|---|---|---|
| `fj` + hint | `fj`, logged in | One line naming `fj` (below) | Main baseline |
| shim + hint | `gh` with the shim, logged in | The README's two-line tip (below) | Main comparison |
| `fj` bare | `fj`, logged in | None | Second table row |
| shim bare | `gh` with the shim, logged in | None | Second table row |

Both instruction files carry the same text, because Claude Code reads `CLAUDE.md` and Codex reads
`AGENTS.md`.

The `fj` hint:

```text
This repository is on Forgejo at <host>. `fj` is installed and logged in. Use it for pull requests, checks, reviews and comments.
```

The shim hint is the README's
[`CLAUDE.md` tip](../../README.md#tell-claudes-agent-to-use-gh), word for word:

```text
`gh` works in this repository through gh-forgejo-shim. Use it for pull requests, checks, reviews and comments.
Pass `--hostname <your forge host>` to every `gh api` call; without it, `gh api` can go to github.com.
```

The hinted pair is the headline. It matches what a careful user sets up, and the README already
tells Claude users to add the tip, because Claude's agent otherwise assumes `gh` only works with
GitHub. The bare pair shows what happens with no setup at all. It runs only if the hinted pair
finishes within budget.

`tea` is out. Gitea archived it on 2026-10-29 and replaced it with `gt`, whose README doesn't
mention Forgejo. Testing either would measure Gitea compatibility, not the question above. A
footnote under the table says so.

## What it measures

| Measure | Definition |
|---|---|
| **Tasks done** | The task's checker reads Forgejo's API after the run and finds the end state the task asked for. For a question, the answer contains the facts the checker lists. The agent's own claim of success counts for nothing. |
| **Failed forge calls** | Calls to `gh`, `fj` or `curl` against the forge that exit non-zero. |
| **Help lookups** | `--help`, `help` subcommands, `man`, and reads of a CLI's source or docs. |
| **Tool calls** | Every tool call in the agent's log, of any kind. |
| **Tokens** | Input and output tokens from the agent's log. The results file keeps cached input as its own column. |
| **Time** | Wall-clock time from start to the agent's exit. |

Two more go in the results file and in a sentence under the table, not in columns:

- **Fell back to raw API:** the agent stopped using the arm's CLI and called the forge with `curl`.
- **Reported done, but wrong:** the agent said it finished, and the checker disagrees. For example,
  it commented on the wrong pull request. Each one is also a failed task.

**How calls are counted.** A transcript can't show the exit code of a command inside a pipe
(`fj pr view 3 | head`), so the harness doesn't rely on it. Small wrappers placed ahead of `gh`,
`fj` and `curl` on `PATH` log each call's arguments, exit code and duration to a JSONL file, then
exit with the real program's code. The forge call counts come from that file. Tool calls, tokens
and time come from the agent's own log.

**Limits.** Each run stops after 15 minutes or 60 tool calls. A run that hits either limit counts
as a failed task, and its counts stay in the results.

## Tasks

Ten tasks, picked from the `gh` commands agents ran in recorded sessions (the list in
[`scripts/e2e/agent-commands.sh`](../../scripts/e2e/agent-commands.sh)) and from the README's
[What works](../../README.md#what-works). Each task has a fixed prompt, a seeded starting state and a
checker. The prompts never name a tool.

| # | Prompt (short form) | Checker |
|---|---|---|
| 1 | Is the PR for this branch ready to merge? | Answer names the failing check and the open review comment |
| 2 | Which checks failed on PR #N, and what do they say? | Answer names each failed check and its description |
| 3 | Reply to the review comment on `src/cli/commands.ts` line 45 | A reply exists in that comment's thread, from the agent's user |
| 4 | Open a PR from this branch into `main` with this title and body | One open PR, this head and base, title and body match |
| 5 | Merge PR #N with a rebase once its checks pass | PR merged, rebase style, after the checks went green |
| 6 | File an issue for this bug and label it `bug` | One new issue with that label |
| 7 | Add a comment to issue #M saying the fix is in PR #N | One new comment on #M that links #N |
| 8 | List the open PRs that `reviewer` has to review | Answer lists exactly the seeded set |
| 9 | Check out PR #N and run its tests | Local branch at PR #N's head; the test command ran |
| 10 | Close PR #N with a comment explaining why | PR closed, not merged, with one new comment |

Task 5's checks turn green a short while after the run starts, so an agent that merges without
waiting fails. Two of the ten sit next to the shim's
[known gaps](../../README.md#known-gaps): task 2 invites `gh run view --log`, which the gateway
doesn't serve yet, and task 4 runs into `gh pr create`'s push check. They stay in so the table
doesn't only show the shim's strengths.

The prompts are frozen after the dry run (phase 2) and stored with the results.

## Runs and reporting

Agents behave differently from run to run, so each task runs 5 times per arm per agent:

- Hinted pair: 10 tasks × 2 arms × 5 runs × 2 agents = 200 runs.
- Bare pair, if run: another 200.

Each run gets a freshly seeded repository. Runs of different arms interleave, so a slow afternoon
at a model provider doesn't land on one arm.

The README shows one row per agent and arm, with counts rather than bare percentages, so a reader
can judge how much is noise. Example shape only:

| Agent | Tool | Tasks done | Failed forge calls | Help lookups | Tokens | Time |
|---|---|---|---|---|---|---|
| Claude Code | `gh` + shim | n/50 | median per task | median per task | median per task | median per task |
| Claude Code | `fj` | n/50 | … | … | … | … |
| Codex CLI | `gh` + shim | n/50 | … | … | … | … |
| Codex CLI | `fj` | n/50 | … | … | … | … |

Under it: the run date, the agent, model, shim, `fj`, `gh` and Forgejo versions, a link to the
results file, and a one-line command to rerun it.

The results file, `docs/benchmark/results-YYYY-MM-DD.md`, breaks the table down by task and links
the raw logs. The harness scrubs the logs before committing them, and they hold only the disposable
forge's data.

## The harness

What exists: [`scripts/e2e.sh`](../../scripts/e2e.sh) builds a test image with `gh` and the shim,
starts a throwaway Forgejo in Docker and seeds it, and
[`agent-commands.sh`](../../scripts/e2e/agent-commands.sh) seeds a realistic PR with commits, files
and inline review comments.

What to add, under `scripts/bench/`:

1. **Image.** The e2e image plus Node, Claude Code, Codex CLI and `fj`, each at a pinned version.
   The image ships no forge credentials except the disposable forge's test token.
2. **Seeds.** One seed per task, reusing the e2e seeding code. Each run starts from a fresh copy.
3. **Call log.** The `gh`, `fj` and `curl` wrappers described above.
4. **Runner.** Takes agent, arm, task and run number. Writes the instruction files for the arm,
   runs the agent headless with permission prompts off (safe only because the container and forge
   are disposable), and saves both logs.
5. **Checkers.** One per task, reading Forgejo's API, with unit tests against recorded API answers.
6. **Report.** Reads every run's logs and writes the results file and the README table.

The parsers for the call log and agent logs, and every checker, get tests first. A checker that
passes a broken run makes the whole table worthless.

Model credentials enter the container as environment variables at run time and never touch the
image or the logs.

## Phases

0. **Pilot.** Read existing agent transcripts that ran `fj` against a Forgejo repository and count
   failed `fj` calls and their causes: wrong flags, missing commands, wrong host. The tasks in those
   sessions differ, so the pilot doesn't go in the README. It sizes the gap and shapes the task
   list. Only a scrubbed summary of counts and error kinds goes in this repository.
1. **Harness,** test-first, as above.
2. **Dry run.** One run per task, arm and agent (40 runs). Fix harness bugs and unclear prompts,
   then freeze the prompts.
3. **Hinted pair,** 200 runs.
4. **Bare pair,** 200 runs, if budget allows.
5. **README.** Add the table and a short paragraph near the top, and rewrite the opening to give
   both reasons to install the shim: GUI apps show Forgejo pull requests, and agents get their work
   done with the `gh` they already know. If the results don't show a clear gap, the README says
   that instead.
6. **Reruns.** Before each minor release, or when either agent's major version changes. A table
   older than three months gets rerun or comes down.

## Open decisions

- **Budget.** 200 runs for the hinted pair, 400 with the bare pair. The dry run's token counts give
  a cost estimate before phase 3 starts.
- **Models.** Each CLI's default model (as planned), or also a smaller, cheaper model. Smaller
  models may struggle more with an unfamiliar CLI, but each extra model doubles the runs.
- **Grading answers.** Tasks 1, 2 and 8 check an answer's text for listed facts. If that proves
  brittle in the dry run, the prompt can ask the agent to end with a fixed-format line, which the
  checker reads instead.
