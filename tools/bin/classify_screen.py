#!/usr/bin/env python3
"""Classify a screen hardcopy snap. Prints JSON to stdout.

Usage: classify_screen.py SNAP_FILE [--session NAME]
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import List, Optional, Tuple


def clean_lines(text: str) -> List[str]:
    lines = []
    for raw in text.splitlines():
        line = raw.replace("\x00", "")
        line = re.sub(r"\[\s*SCREEN:\s*[^\]]+\]", "", line)
        # drop other common control leftovers for matching
        line = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f]", "", line)
        lines.append(line.rstrip())
    return lines


def detect_kind(text: str, session: str) -> Tuple[str, float, List[str]]:
    signals: List[str] = []
    low = text.lower()
    scores = {"grok": 0.0, "codex": 0.0, "claude": 0.0}

    if re.search(r"Grok\s+\d", text) or "always-approve" in low:
        scores["grok"] += 0.6
        signals.append("grok_banner")
    if "plan approval" in low and "a:approve" in low:
        scores["grok"] += 0.2
        signals.append("grok_plan_approval_ui")
    if session and "grok" in session.lower():
        scores["grok"] += 0.35

    if "ask codex to do anything" in low or re.search(r"GPT-\d", text):
        scores["codex"] += 0.7
        signals.append("codex_prompt")
    if re.search(r"\? \d+ question", text) or "alt+↑" in text:
        scores["codex"] += 0.25
        signals.append("codex_queued_q")
    if "yolo" in low:
        scores["codex"] += 0.1
        signals.append("yolo_mentioned")
    if session and "codex" in session.lower():
        scores["codex"] += 0.35

    if "bypass permissions" in low:
        scores["claude"] += 0.55
        signals.append("claude_bypass")
    if re.search(r"\d+/\d+ agents done", text) or "/workflows" in text or "dynamic workflow" in low:
        scores["claude"] += 0.3
        signals.append("claude_workflow_ui")
    if "keep going, ping codex" in low:
        scores["claude"] += 0.2
        signals.append("claude_ping_codex_hint")
    if session and "claude" in session.lower():
        scores["claude"] += 0.35

    kind = max(scores, key=scores.get)
    conf = scores[kind]
    if conf < 0.35:
        return "unknown", 0.2, signals
    return kind, min(1.0, conf), signals


PR_PATTERNS = [
    r"https?://github\.com/[^\s]+/pull/\d+",
    r"\bpull request\s+#?\d+\b",
    r"\bPR\s*#?\d+\b",
]


def extract_pr_refs(text: str) -> List[str]:
    found: List[str] = []
    for pat in PR_PATTERNS:
        for m in re.finditer(pat, text, re.IGNORECASE):
            ref = m.group(0).rstrip(".,);]")
            if ref not in found:
                found.append(ref)
    return found[:8]


def recent_blocks(lines: List[str]) -> Tuple[str, str, str]:
    """Return (tail60_lower, tail60_raw, last15_raw) from non-empty lines."""
    nonempty = [ln for ln in lines if ln.strip()]
    tail60 = nonempty[-60:]
    last15 = nonempty[-15:]
    return (
        "\n".join(tail60).lower(),
        "\n".join(tail60),
        "\n".join(last15),
    )


def classify_state(text: str, kind: str, lines: List[str]) -> Tuple[str, float, List[str], Optional[str]]:
    signals: List[str] = []
    low = text.lower()
    tail_low, tail_raw, last15 = recent_blocks(lines)
    last15_low = last15.lower()

    approval_hint: Optional[str] = None

    # --- active approval: prefer LAST 15 lines (current UI), not ancient scrollback ---
    active_approval_cues = [
        r"waiting on plan approval",
        r"\ba:approve\b",
        r"allow once",
        r"allow always",
        r"always allow",
        r"do you want to",
        r"run this command",
        r"permission required",
        r"approve this",
        r"\(y/n\)",
        r"press enter to confirm",
        r"\b1\.\s*allow once\b",
        r"\b2\.\s*allow always\b",
    ]
    needs_approval = False
    for pat in active_approval_cues:
        if re.search(pat, last15_low, re.IGNORECASE):
            needs_approval = True
            signals.append(f"approval:{pat}")
            break
    # Slightly wider window if footer-ish approval chrome is present without mode banner
    if not needs_approval:
        for pat in [r"waiting on plan approval", r"\ba:approve\b", r"allow once", r"\(y/n\)"]:
            if re.search(pat, tail_low, re.IGNORECASE) and not re.search(
                r"always-approve|bypass permissions on", last15_low
            ):
                # Only if the cue is near the end of tail60 (last ~25 lines of nonempty)
                near = "\n".join([ln for ln in lines if ln.strip()][-25:]).lower()
                if re.search(pat, near, re.IGNORECASE):
                    needs_approval = True
                    signals.append(f"approval_near:{pat}")
                    break

    if needs_approval:
        if kind == "grok" or re.search(r"a:approve|waiting on plan approval", last15_low):
            approval_hint = "a"
        elif kind == "claude" or re.search(r"allow always|2\.\s*allow", last15_low):
            approval_hint = "2"
        elif re.search(r"always allow|allow always", last15_low):
            approval_hint = "always"
        else:
            approval_hint = "y"

    # --- activity ---
    actively_working = bool(
        re.search(r"working\s*\(\d", text, re.IGNORECASE)
        or re.search(
            r"task started|monitor still running|\d+ monitor|"
            r"waiting for \d+ dynamic workflow|running in background|"
            r"\d+/\d+ agents done|"
            r"background terminal running",
            last15_low,
        )
        or re.search(
            r"task started|monitor still running|working\s*\(\d|"
            r"waiting for \d+ dynamic workflow|running in background",
            tail_low,
        )
    )
    if actively_working:
        signals.append("actively_working")

    # --- needs_input (current) ---
    needs_input = False
    if re.search(r"\? \d+ question", last15):
        needs_input = True
        signals.append("queued_question")
    if re.search(r"waiting for (your )?input|waiting for human|human approval|awaiting (your )?response", last15_low):
        needs_input = True
        signals.append("waiting_for_input")
    # Bare Ask Codex is idle cue, not needs_input by itself

    # --- pr_blocked: current wait, not historical ownership text ---
    pr_blocked = False
    # Strong current cues in last ~25 lines
    near25 = "\n".join([ln for ln in lines if ln.strip()][-25:]).lower()
    near25_raw = "\n".join([ln for ln in lines if ln.strip()][-25:])
    current_pr_cues = [
        r"ci (is )?queued",
        r"hosted run \d+ is queued",
        r"ci (is )?fail",
        r"merge blocked",
        r"review requested",
        r"waiting for (the )?pr\b",
        r"waiting (on|for).*ci\b",
        r"watch pr \d+ until ci",
        r"1 monitor still running",  # grok watching after PR open
    ]
    for pat in current_pr_cues:
        if re.search(pat, near25, re.IGNORECASE):
            pr_blocked = True
            signals.append(f"pr:{pat}")
            break

    # Historical ownership / handoff text alone should NOT flip state if agent is working
    hist_pr = bool(
        re.search(
            r"pending handoff|checkout ownership|blocked by the benchmark|applying it and completing ci",
            low,
        )
    )
    if hist_pr:
        signals.append("pr_history_mention")
        if not actively_working and not pr_blocked:
            # Only treat as pr_blocked if also currently idle at prompt after that message
            if re.search(r"ask codex to do anything|build anything", last15_low):
                pr_blocked = True
                signals.append("pr:idle_after_handoff_block")

    waiting_grant = bool(
        re.search(r"waiting on (a )?cpu grant|still no grant from|ping codex again", near25)
        or re.search(r"keep going, ping codex", last15_low)
    )
    if waiting_grant:
        signals.append("waiting_codex_grant")

    is_error = bool(
        re.search(r"\bfatal error\b|\bpanic!\b", last15_low)
        and not actively_working
    )

    idle_prompt = bool(
        re.search(r": ask codex to do anything", last15_low)
        or re.search(r"build anything", last15_low)
    )
    # Fresh idle input chrome in last15 overrides stale "Task started" in scrollback
    fresh_idle_chrome = bool(
        re.search(r"\bbuild anything\b", last15_low)
        or re.search(r": ask codex to do anything", last15_low)
    )
    fresh_busy = bool(
        re.search(
            r"working\s*\(\d|waiting for response|monitor still running|"
            r"\\d+/\\d+ agents done|pouncing|esc to interrupt",
            last15_low,
        )
    )
    if fresh_idle_chrome and not fresh_busy and not needs_approval:
        actively_working = False
        if "actively_working" in signals:
            signals = [s for s in signals if s != "actively_working"]
        signals.append("fresh_idle_chrome")

    # Grok always-approve footer with no monitor / no task = could be idle
    grok_idle = (
        kind == "grok"
        and re.search(r"always-approve", last15_low)
        and not actively_working
        and not needs_approval
        and not pr_blocked
    )

    # Priority
    if needs_approval:
        return "needs_approval", 0.88, signals, approval_hint
    if is_error:
        return "error", 0.6, signals, approval_hint
    if needs_input and not actively_working:
        return "needs_input", 0.8, signals, approval_hint

    # Actively watching CI / PR → pr_blocked (nudge-eligible) even while "working"
    if pr_blocked:
        # If clearly mid-flight on non-watch work (Working Ns without CI watch), prefer working
        if actively_working and re.search(r"working\s*\(\d", text, re.IGNORECASE):
            # Codex "Working" means busy — don't nudge mid-flight
            if not re.search(r"ci (is )?queued|watch pr|hosted run \d+ is queued|monitor still running", near25):
                return "working", 0.82, signals, approval_hint
        return "pr_blocked", 0.72, signals, approval_hint

    if actively_working:
        return "working", 0.85, signals, approval_hint

    if waiting_grant and not actively_working:
        return "idle", 0.7, signals, approval_hint

    if idle_prompt or grok_idle:
        return "idle", 0.6, signals, approval_hint

    if waiting_grant:
        return "idle", 0.65, signals, approval_hint

    return "unknown", 0.35, signals, approval_hint


def make_summary(kind: str, state: str, lines: List[str], pr_refs: List[str]) -> str:
    nonempty = [ln.strip() for ln in lines if ln.strip()]
    interesting = []
    skip = re.compile(
        r"Grok \d|Ask Codex|bypass permissions|Shift\+Tab|GPT-\d|ctrl\+[a-z]",
        re.I,
    )
    for ln in reversed(nonempty[-40:]):
        if len(ln) < 8 or skip.search(ln):
            continue
        interesting.append(ln)
        if len(interesting) >= 4:
            break
    interesting.reverse()
    snippet = " | ".join(interesting)[:280]
    pr = (", ".join(pr_refs[:3]) if pr_refs else "none")
    return f"[{kind}/{state}] PRs: {pr}. Recent: {snippet}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("snap_file")
    ap.add_argument("--session", default="")
    args = ap.parse_args()

    path = Path(args.snap_file)
    if not path.exists():
        print(json.dumps({"error": f"missing snap: {path}", "session": args.session}))
        return 1

    raw = path.read_text(encoding="utf-8", errors="replace")
    lines = clean_lines(raw)
    text = "\n".join(lines)
    session = args.session or path.stem

    kind, kind_conf, kind_signals = detect_kind(text, session)
    state, state_conf, state_signals, approval_hint = classify_state(text, kind, lines)
    pr_refs = extract_pr_refs(text)

    confidence = round(min(1.0, 0.4 * kind_conf + 0.6 * state_conf), 3)
    signals = kind_signals + state_signals

    nonempty = [ln for ln in lines if ln.strip()]
    tail = "\n".join(nonempty[-20:])

    out = {
        "session": session,
        "kind": kind,
        "state": state,
        "confidence": confidence,
        "signals": signals[:24],
        "approval_hint": approval_hint,
        "pr_refs": pr_refs,
        "summary": make_summary(kind, state, lines, pr_refs),
        "tail": tail,
    }
    json.dump(out, sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
