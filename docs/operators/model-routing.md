# Codex model routing for development

New simple or precisely specified development tasks start with `gpt-6-luna`
at `medium` reasoning effort. The local `tools/codex-luna.sh DIR` launcher
accepts one existing working directory and pins those settings for a fresh
interactive Codex session. It does not change user configuration or provide
an escalation bypass. Provider permissions continue to come from the user's
Codex configuration.

Escalate only after an objective failure at the current tier: retain the
failing command and output, write a small handoff with the attempted approach
and evidence, then route the next attempt according to the repository's
evidence-based model-routing playbook. When that task ends, reset the next task
to Luna medium. At an existing session's model picker, `s` changes that
session only; make the switch at an idle boundary. Never edit shared/global
Codex configuration to route a lane.

The requested model in a launch command is not an observation of the provider
identity that actually served a response. This launcher only sets development
session defaults; using it does not demonstrate native execution, managed
provider qualification, production readiness, or acceptance.
