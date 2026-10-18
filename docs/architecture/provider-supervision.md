# Provider supervision prototype

This document describes a pure component prototype. It accepts typed snapshots, seven separately retained clocks, a persisted detector state and an explicit caller-supplied tick. It returns a classification and optional evidence request. It does not observe a provider, establish process identity, signal a process, send input, grant execution, or accept work.

All seven clocks are independent evidence. Missing or future data cannot establish health. A long-running tool heartbeat is classified as waiting while the tool runs; silence alone does not establish a model stall. A suspect-stall proposal requires at least two stale clocks and repeated identical samples according to the versioned hysteresis policy. New clock progress resets the repeated-sample count. Cooldown uses explicit ticks and checked arithmetic.

Approval observations are typed evidence only. The text is not interpreted as authority, and every result keeps `may_signal`, `may_send_input` and `execution_granted` false. Quoted, historical, negated, or example text cannot authorize an action through this interface. There is no regex-only universal stall guarantee.

This prototype has no qualified platform/provider tuples. It is not a live qualification, provider capability claim, process-control interface, or acceptance receipt. Provider wait and unknown process/tool states remain conservative classifications pending independent evidence.
