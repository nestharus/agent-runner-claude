# Agent Runner Claude Provider — Agent Entry Point

Read `/home/nes/ai/AGENTS.md` for shared routing, dispatch and the landable
change lifecycle; this file adds only repository rules.

## Purpose

This repository owns the Claude Code terminal adapter for Agent Runner's
versioned `oulipoly.provider/v1` external-provider contract. Provider-neutral
behavior, including the shared one-shot launch lifecycle, belongs in
`agent-provider-sdk`; Claude command, policy, quota, transcript/session and
terminal interpretation belong here. Plug Claude behavior into the SDK
lifecycle rather than reimplementing its ordering. Do not use the Claude Agent
SDK here.

## Repository layout

The integration checkout is `~/projects/agent-runner-claude/trunk`. Do work in
`~/projects/agent-runner-claude/worktrees/<ticket-or-task>` branched from
remote `main`, merge verified work to remote `main`, then remove the task
worktree and branch. Do not overwrite or delete unrelated branches or
worktrees.

## Runtime compatibility

- Determine runtime compatibility from declared supported wire schemas and
  capability agreement. Native CLI banners, SDK source revisions, package
  versions and executable byte identity must not be equality requirements.
- Consume SDK source without an explicit manifest source-revision constraint.
  `Cargo.lock` records resolved builds; update it through Cargo tooling.
- Compatible provider rebuilds and updates must remain usable automatically.
  Keep binary/source identity out of request digests and preserve compatible
  durable launch state and completed replay.
- Preserve actor recovery safeguards and reconciliation of incomplete work.

Current integration uses fixed-v1 validation and capability agreement; do not
advertise a second wire version before the host can select a common supported
version.

## Tests

Use deterministic fake native commands; no live model is needed to test
plumbing. Live Claude checks need an explicit low-cost route.
