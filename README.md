# agent-runner-claude

Standalone Claude provider CLI for the `oulipoly.provider/v1` external provider
contract.

This provider implements the one-shot invocation convention:

```text
agent-runner-claude <subcommand>
```

Each subcommand reads one JSON request envelope on stdin. Non-launch commands
write one JSON response envelope on stdout. `launch` writes newline-delimited
JSON events and finishes with an `exit` event.

Implemented commands:

- `describe`
- `schema`
- `policy.evaluate`
- `terminal.classify`
- `launch`
- `resident.prepare` and `resident.serve` (see Resident sessions)

Example:

```bash
printf '%s' '{"contract":"oulipoly.provider/v1","request_id":"req-1","host":{"app":"test"},"params":{}}' \
  | agent-runner-claude describe
```

## Host-selected extensions

`describe` advertises an extension capability only when the request's own
`host.env` selected it: `launch_output_v1` for
`OULIPOLY_HOST_LAUNCH_OUTPUT_V1=1` and `resident_session_v1` for
`OULIPOLY_HOST_RESIDENT_SESSION_V1=1`. An offer of another resident version
alone advertises nothing. Contract definitions and conformance validation come
from the resolved `agent-provider-contract` crate; this repository carries no
private wire-schema snapshot. The lock resolves SDK 0.3.0 at `99ad1183` as build
provenance, without a manifest revision qualifier or runtime identity check.

The adapter still has an older `session.read_turns` implementation returning
`turns`, `turn_count` and `complete`. It does not implement the SDK's bounded
`oulipoly.session_turn_pages/v1` shape and does not advertise
`session_turn_pages_v1`. The shared-schema conformance test exposes this mismatch;
SDK uptake is incomplete until that capability receives a separate implementation
or disposition. Do not treat the older response as shared-contract conformance.

**Launch output (`oulipoly.launch_output/v1`).** A launch whose params carry
`output_delivery: {"protocol": "oulipoly.launch_output/v1"}` is admitted only
when its host selected the extension; otherwise it is refused
(`launch_output_not_selected`) before any native effect, never ignored. A
selected launch ends every exit path (native exit or cancellation, an observed
start failure, and a launch settled without a native process) with an
`oulipoly.launch_output_complete/v1` marker as the last event before `exit`:
byte counts and SHA-256 of all `stdout` and `stderr` data events and their
count, which Agent Runner's output spool checks against what it received. The
marker is journaled and replayed with the launch. Without `output_delivery` the
events are unchanged.

## Resident sessions (`oulipoly.resident_session/v1`)

`resident.prepare` (params: the SDK extension's `protocol` and `launch`
template, i.e. the Claude Code CLI argv that `policy.evaluate` produced, its
environment and route) refuses session-selecting flags (`--resume`, `-r`,
`--continue`, `-c`, `--session-id`, `--fork-session`), drops the template's own
transport flags (`-p`/`--print`, `--verbose`, `--replay-user-messages`,
`--output-format`, `--input-format`), records the template durably and
content-addressed under
`<data_root>/provider-state/claude/resident/configs/<sha256>.json`, and answers
`invocation.args = ["resident.serve", "--config", <path>]` with
`endpoint: "stdio"`, which the host appends to this same registered executable.

`resident.serve` is the SDK's resident ACP v2 endpoint. Each turn runs the
native Claude Code CLI once through the SDK lifecycle, as
`<template> -p --input-format stream-json --output-format stream-json --verbose
--replay-user-messages`, with `--session-id <uuid>` (an id this provider
chooses) on the session's first turn and `--resume <uuid>` afterwards, in the
session's working directory, writing one stream-JSON user message on stdin. The
`system/init` event names the native session; the replayed user message (its
`uuid`, message content/role, session and no tool parent) is the consumption that acknowledges the prompt; each
main-thread assistant message with text is one ACP `agent_message` (subagent and
tool-use records are not); the turn succeeds only with a `result` of subtype
`success` and no error, otherwise an exit 0 is reported as exit 1. Stderr is
accounted and kept for terminal classification. `session/cancel` terminates only
that turn's process group. The native stream-JSON behaviour is exercised here
against a deterministic fake CLI only (`tests/claude_resident.rs`); it has not
been qualified against a live Claude Code CLI. Never the Claude Agent SDK.

## Host tool mediation (`oulipoly.tool_mediation/v1`)

`describe` advertises `tool_mediation_v1` only when the request's `host.env`
selected `OULIPOLY_HOST_TOOL_MEDIATION_V1=1`. A resident template whose
environment carries the host's policy (`OULIPOLY_TOOL_MEDIATION_V1`: a Bash allow
list or `trusted-task`, the host's requester and the name of its Bash ingress
variable) makes the SDK's mediated `bash` tool Claude Code's only command tool on
every turn: `--strict-mcp-config --mcp-config` with one server, `oulipoly`
(this provider executable's `tool.bridge` subcommand, with the policy and the
ingress value in its own `env`), so its tool is `mcp__oulipoly__bash`;
`--tools ""` (allow list) or `--tools Read,Write,Edit` (`trusted-task`, the
embedded receiver's meaning); `--allowedTools` naming exactly those tools;
`--disallowedTools` naming built-in `Bash`, delegation, background, web, notebook
and skill tools; `--permission-mode dontAsk`; `--setting-sources ""`;
`--disable-slash-commands`; and `ENABLE_TOOL_SEARCH=false`. The bridge refuses a
command outside an allow list and starts no requester; otherwise it runs only
`requester run --delivery sync|async -- bash -lc COMMAND`, so the command reaches
the root's own Bash ingress.

The shared bridge uses ordered durable `accepted` / `started.exec_error` stage
evidence to report a failed program exec with its diagnostic and `isError: true`.
It retains accepted custody, possible setup effects and the warning against
replay. A failed async exec does not claim the program is running; a later
completion is claimed only with the matching acceptance, detach and output
reference. Wait status and output proof remain separate facts. Exit code 127
alone is an ordinary numeric wait, not proof of failed exec. Historical requester
results with the same stage evidence work without a new schema or version gate.
Finite checks of rebuilt provider bridges with collected results and stand-in
requesters qualify rendering only; an all-real bridge-originated failed shell
exec and real native tool efficacy remain unqualified. Internally inconsistent
requester objects can conservatively render unresolved and lose a diagnostic.

`policy.evaluate` admits the policy strictly and reports it as an
`oulipoly.tool_mediation/v1` marker; an invalid policy, a host selection
without one, `tool_restrictions`, or a template carrying its own tool,
permission, settings, agent, plugin, directory or MCP options is
`accepted: false`, and `resident.prepare` refuses the same. A turn whose
serving process lacks the named ingress variable is refused
(`tool_mediation_ingress_unavailable`) before Claude Code starts. One-shot
`launch` does not apply mediation and refuses a policy
(`tool_mediation_resident_only`). Coverage: `tests/claude_tool_mediation.rs`
(fake Claude Code starting the configured MCP server with only PATH, HOME and
the server's own `env`; requester stand-in; stand-in ingress socket). Whether a
real Claude Code release honours each constructed option remains to be
qualified.

## Child exploration (`oulipoly.exploration/v1`)

`describe` advertises `exploration_v1` only when the request's `host.env`
selected `OULIPOLY_HOST_EXPLORATION_V1=1`. A mediated resident template whose
environment also carries the host's offer (`OULIPOLY_EXPLORATION_V1`: opaque
route labels, the host's child requester and the name of its owner ingress
variable) is admitted with the SDK's `exploration::admit`. Three kinds of
offer are refused rather than ignored, by `policy.evaluate` (`accepted:
false`), `resident.prepare` and `resident.serve`: one the request's host did
not select, one without a tool mediation policy, and an invalid one. One-shot
`launch` takes no mediation, so it refuses an offer too.

An admitted offer adds one tool and nothing else. The `oulipoly` server's own
`env` also carries the offer and its ingress value, so the same bridge serves
the SDK's non-command `explore` tool beside `bash`. `--allowedTools` adds
`mcp__oulipoly__explore`. `policy.evaluate` reports the
`oulipoly.exploration/v1` marker with that native name, and the tool-mediation
marker lists it in `native_tools`. The `system/init` inventory check expects
it, so a report that omits it refuses the turn. The owner admits or refuses
each child. A turn whose process lacks the offer's ingress variable is refused
(`exploration_ingress_unavailable`) before Claude Code starts.

Without an admitted offer, `bash` stays alone. Claude Code's own environment
drops `OULIPOLY_EXPLORATION_V1`, so an inherited variable of that name is never
an offer, whatever Claude Code passes to MCP servers. Coverage:
`tests/claude_tool_mediation.rs`, with the same fakes plus a child requester
stand-in. These fakes show the provider's configuration only, not that a real
Claude Code offers exactly these tools, nor that a model uses `explore` well.

## Launch lifecycle

`launch` runs through the shared one-shot lifecycle (`lifecycle::run_launch`)
of the `agent-provider-execution` crate in
[agent-provider-sdk](https://github.com/nestharus/agent-provider-sdk). The SDK
owns request custody, replay, reconciliation, admission, the native effect
gate, process-group custody, draining, heartbeats, cancellation and the host
deadline, the `exit` event and the completion receipt. This adapter supplies
the request digest (launch params, host environment and host working
directory), the host-supplied argv/environment/stdin, raw byte framing of
native output, the `provider_session_known` marker, Claude terminal
classification and failure codes.

- Launch custody lives under `<host.data_root>/provider-state/claude/launch`
  (default `$HOME/.local/share/oulipoly-agent-runner`). An identical retry of a
  completed request ID replays its events byte-for-byte without running the
  command again; reusing a request ID with different inputs fails with
  `request_changed`. A retry after the provider was lost mid-launch terminates
  the recorded native process group and fails with
  `launch_reconciliation_required` rather than running the command again; use a
  new request ID after reconciling.
- `host.deadline_unix_ms` is honored: a deadline that elapses before the
  native command is admitted, including while the launch is being prepared,
  refuses the launch with `launch_deadline` (category `timeout`) before any
  native effect and without consuming the request ID; a deadline reached
  during the run terminates the native process group and reports a `cancelled`
  exit. `SIGTERM`/`SIGINT` to the provider do the same (`launch_cancelled`
  before admission). The `cancelled` exit does not distinguish deadline from
  signal.
- The native command leads its own process group. When its leader exits, the
  remaining group is terminated and output is drained, so descendants holding
  output pipes cannot strand completion. Output that closes without the
  command exiting, or that stays open and silent after termination, fails the
  launch after a two-second grace and leaves it for reconciliation.
- Commands that cannot be spawned are reported as a `spawn_error` exit with
  the actual OS error and recorded like any other outcome: empty argv and
  invalid stdin encoding before any process starts, and otherwise the SDK's
  observed failure to start the command in its working directory or to `exec`
  it (not found on the request's `PATH`, which is resolved in that directory,
  not executable, or a missing interpreter). Nothing predicts this from
  `PATH` or permission bits. A command that runs and exits 126 remains a
  native `exited` result, whatever it writes to stderr.
- Incomplete stdin delivery to a command that exits 0 fails with
  `stdin_failed` and leaves the launch for reconciliation rather than
  reporting success.
- Launch output goes through the SDK's `BoundedOutput`: writes to a FIFO or
  socket fail after two seconds without progress, which bounds a stalled
  reader rather than total delivery time; regular files and other descriptors
  have no write bound. The provider exits 0 after successful launch completion,
  including exit delivery, journal sealing and completion-state publication,
  for native nonzero exits, spawn errors and cancellation as well. Callers
  must read the `exit` event for the native outcome. A failure after events
  began attempts to append one `ok: false` error envelope (not a launch event,
  without a trailing newline) after the events already delivered and returns
  a failure invocation code. A seal or state-publication failure can occur
  after `exit` delivery, so an error envelope can follow an `exit` event; if
  output delivery itself failed, no envelope can follow. A delivered `exit`
  event does not by itself prove successful invocation or durable completion.

Cargo resolves SDK source from its public GitHub repository without a
manifest source-revision constraint. `Cargo.lock` records the resolved commit
for reproducible builds; it is not a runtime compatibility pin. Use
`cargo update -p agent-provider-execution` to refresh it, then verify the
adapter. The provider is Linux-only.

Resident correction: queued inputs bind native create/resume at dispatch from
settled state. Consumption evidence with failed insertion persistence returns
unknown (`-32011`), without ACK. Interrupted actors are discharged even if the
current template changes, and are never rerun; unsettled custody cannot claim
ended/close completion. Close releases its lock and worker. Dedup ACKs attest
the original key only, not current prompt bytes. Describe admits future
advertisements through the SDK's typed admission and common-version chooser,
with declared preference and strict selected v1 payloads. Host replacement
refresh, installed/native qualification and actual Runner joining remain open.
The SDK's v1 snapshot realignment replaces the former session page shape;
it is not a compatible evolution or a wire major version. Other hosts are
unqualified. Per-request terminal-unavailable selection remains required.

Claude template parsing consumes known value-bearing options before interpreting
resident-owned flags. Values such as `--append-system-prompt --resume` or `-p`
are preserved. Unknown option arity is explicitly refused at prepare; extend
the adapter's option table for new native options. `system/init` must have a
UUID-shaped id equal to the selected native session; drift is a turn failure.
`isReplay` alone never acknowledges input. Assistant records with a tool parent
or `isSidechain: true` are excluded. Submitted user UUIDs have v4/variant bits.
These assumptions have only deterministic fake qualification; the actual CLI
echo/create/resume semantics must be checked before installed cutover.

For mediated resident turns, `system/init` tool/MCP inventory is compared with the
constructed policy. Present contradictory or malformed inventory refuses the
turn with `native_tool_inventory_mismatch`. Every supplied init records
`claude.native_tool_inventory` in the durable native-turn journal, including
expected/reported facts and absent fields as `not-reported`. Consistent reports
are observations, not enforcement attestation; missing fields permit the turn
without attesting native efficacy. Installed Claude recognition and enforcement
remain qualification work.
