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
