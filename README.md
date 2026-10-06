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
- `host.deadline_unix_ms` is honored: an elapsed deadline refuses the launch
  with `launch_deadline` before any native effect, and a deadline reached
  during the run terminates the native process group and reports a `cancelled`
  exit. `SIGTERM`/`SIGINT` to the provider do the same.
- The native command leads its own process group. When its leader exits, the
  remaining group is terminated and output is drained, so descendants holding
  output pipes cannot strand completion. Output that closes without the
  command exiting, or that stays open and silent after termination, fails the
  launch after a two-second grace and leaves it for reconciliation.
- Commands that cannot be spawned (empty argv, invalid stdin encoding, missing
  working directory, or a program that is not found or not executable) are
  still reported as a `spawn_error` exit and recorded like any other outcome.
  A program removed between that check and exec surfaces as exit status 126
  from the native effect gate.
- Launch output uses nonblocking writes with a two-second no-progress limit.
  The provider exits 0 once the `exit` event is delivered; failures after
  events began are reported as a final error envelope line.

Cargo resolves SDK source from its public GitHub repository without a
manifest source-revision constraint. `Cargo.lock` records the resolved commit
for reproducible builds; it is not a runtime compatibility pin. Use
`cargo update -p agent-provider-execution` to refresh it, then verify the
adapter. The provider is Linux-only.
