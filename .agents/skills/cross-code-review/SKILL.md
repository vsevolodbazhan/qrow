---
name: cross-code-review
description: Request cross code review from other model and/or harness.
---

# General

- If you are an Anthropic model, prioritize requesting review from OpenAI (Codex) models.
- If you are an OpenAI model, prioritize requesting review from Anthropic (Claude) models.
- Base branch is `main`.
- If you are given a PR to review, checkout its branch locally first.
- Tweak default timeout for big changes.

# Codex

```
gtimeout 600s codex --model gpt-6.1-sol --sandbox read-only review --base main
```

# Claude

```
gtimeout 600s claude --permission-mode auto --model opus --print "Review current changes against the base branch main"
```
