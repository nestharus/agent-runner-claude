# agent-runner-claude

Standalone Claude provider CLI for the `oulipoly.provider/v1` external provider
contract.

This foundation slice implements the one-shot invocation convention:

```text
agent-runner-claude <subcommand>
```

Each subcommand reads one JSON request envelope on stdin and writes one JSON
response envelope on stdout. This slice implements `describe` and `schema`.
Later slices will fill the advertised Claude capabilities.

Example:

```bash
printf '%s' '{"contract":"oulipoly.provider/v1","request_id":"req-1","host":{"app":"test"},"params":{}}' \
  | agent-runner-claude describe
```
