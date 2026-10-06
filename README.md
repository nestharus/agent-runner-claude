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

Example:

```bash
printf '%s' '{"contract":"oulipoly.provider/v1","request_id":"req-1","host":{"app":"test"},"params":{}}' \
  | agent-runner-claude describe
```

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
  have no write bound. The provider exits 0 once the `exit` event is
  delivered, including for native nonzero exits, spawn errors and
  cancellation, so callers must read the `exit` event rather than the provider
  exit code. A failure after events began appends one `ok: false` error
  envelope (not a launch event, without a trailing newline) after the events
  already delivered, with no `exit` event; if output delivery itself failed,
  no envelope can follow. A delivered `exit` event does not by itself prove
  the completion record was published.

Cargo resolves SDK source from its public GitHub repository without a
manifest source-revision constraint. `Cargo.lock` records the resolved commit
for reproducible builds; it is not a runtime compatibility pin. Use
`cargo update -p agent-provider-execution` to refresh it, then verify the
adapter. The provider is Linux-only.
