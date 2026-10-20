import { useEffect, useId, useRef, useState } from "react";
import {
  CHAT_TEXT_BYTES,
  ChatApiError,
  loadOperatorChat,
  mergeChatMessages,
  newProjectChat,
  postOperatorChat,
  retryChatRequest,
  textBytes,
  type ChatMemory,
  type ChatMessage,
  type ChatProjectMemory,
  type ChatRequest,
  type ChatSnapshot,
} from "./operator-chat";

export default function GitChat({
  projectId, projectName, token, memory, active, disabledReason, onAccess, onToggle,
}: {
  projectId: string;
  projectName: string;
  token?: string;
  memory: ChatMemory;
  active: boolean;
  disabledReason: string;
  onAccess: () => void;
  onToggle: (open: boolean) => void;
}) {
  const [record, setRecord] = useState(() => memory.projects.get(projectId) ?? newProjectChat());
  const recordRef = useRef(record);
  const [snapshot, setSnapshot] = useState<ChatSnapshot | null>(null),
    [error, setError] = useState(""),
    [sendingId, setSendingId] = useState("");
  const mounted = useRef(true), sending = useRef(false),
    accessDenied = useRef(false),
    requests = useRef(new Set<AbortController>()), panelId = useId(),
    timelineRef = useRef<HTMLDivElement>(null), followTimeline = useRef(true);
  const update = (change: (prior: ChatProjectMemory) => ChatProjectMemory) => {
    const next = change(recordRef.current);
    recordRef.current = next;
    memory.projects.set(projectId, next);
    setRecord(next);
  };
  const rejectAccess = (message: string) => {
    accessDenied.current = true;
    for (const request of requests.current) request.abort();
    setSnapshot(null);
    setError(message);
    update((prior) => ({ ...prior, messages: [], replies: [], draft: "", targetId: "" }));
  };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      for (const request of requests.current) request.abort();
    };
  }, []);
  useEffect(() => {
    if (!record.open || !active || accessDenied.current) return;
    let stopped = false, timer: ReturnType<typeof setTimeout> | undefined;
    const controller = new AbortController();
    async function poll() {
      const request = new AbortController();
      requests.current.add(request);
      const abort = () => request.abort();
      controller.signal.addEventListener("abort", abort, { once: true });
      const deadline = setTimeout(abort, 12_000);
      try {
        const next = await loadOperatorChat(projectId, token, request.signal);
        if (stopped || accessDenied.current) return;
        setSnapshot(next);
        setError("");
        update((prior) => ({
          ...prior,
          targetId: next.targets.some((target) => target.id === prior.targetId)
            ? prior.targetId
            : (next.targets.find((target) => target.role === "captain") ?? next.targets[0])?.id ?? "",
          messages: mergeChatMessages(next.messages, prior.messages),
          replies: next.responses,
        }));
      } catch (failure) {
        if (stopped || accessDenied.current) return;
        const detail = failure instanceof Error ? failure.message : "Operator chat unavailable";
        setError(detail);
        if (failure instanceof ChatApiError && [401, 403].includes(failure.status)) {
          rejectAccess(detail);
          return;
        }
      } finally {
        clearTimeout(deadline);
        requests.current.delete(request);
        controller.signal.removeEventListener("abort", abort);
      }
      if (!stopped && !accessDenied.current) timer = setTimeout(poll, 3_000);
    }
    void poll();
    return () => {
      stopped = true;
      controller.abort();
      clearTimeout(timer);
    };
  }, [record.open, record.targetId, active, projectId, token]);
  const target = snapshot?.targets.find((item) => item.id === record.targetId),
    bytes = textBytes(record.draft),
    sendBlock = disabledReason || error || (!snapshot ? "Checking live chat destinations…" :
      !snapshot.enabled ? "Operator chat is not configured. Ask the operator to configure protected Admiral / Captain inboxes with --operator-chat-config." :
      !target ? "No live destination configured for this project." : "");
  async function send(request: ChatRequest, isNew: boolean) {
    if (accessDenied.current || sending.current || sendBlock || !active || !target || request.target_id !== target.id) return;
    sending.current = true;
    followTimeline.current = true;
    setSendingId(request.id);
    const optimistic: ChatMessage = {
      ...request, state: "uncertain", created_utc: recordRef.current.messages.find((message) => message.id === request.id)?.created_utc ?? new Date().toISOString(),
      detail: "Delivery is not yet confirmed. A retry preserves this message ID.",
    };
    update((prior) => ({ ...prior, draft: isNew ? "" : prior.draft, messages: mergeChatMessages([optimistic], prior.messages) }));
    const controller = new AbortController();
    requests.current.add(controller);
    const deadline = setTimeout(() => controller.abort(), 12_000);
    try {
      const delivered = await postOperatorChat(request, token, controller.signal);
      if (!mounted.current || accessDenied.current) return;
      update((prior) => ({ ...prior, messages: mergeChatMessages([delivered], prior.messages) }));
      setError("");
    } catch (failure) {
      if (!mounted.current || accessDenied.current) return;
      const detail = failure instanceof Error ? failure.message : "Delivery could not be confirmed";
      const definite = failure instanceof ChatApiError && failure.status < 500;
      update((prior) => ({ ...prior, messages: mergeChatMessages([{
        ...optimistic, state: definite ? "failed" : "uncertain",
        detail: definite ? detail : `${detail}. Check delivery or retry this same message ID.`,
      }], prior.messages) }));
      if (failure instanceof ChatApiError && [401, 403].includes(failure.status)) {
        rejectAccess(detail);
      }
    } finally {
      clearTimeout(deadline);
      requests.current.delete(controller);
      sending.current = false;
      if (mounted.current) setSendingId("");
    }
  }
  const messages = record.messages.filter((message) => message.target_id === record.targetId),
    replies = record.replies.filter((reply) => reply.target_id === record.targetId);
  const timeline = [
    ...messages.map((message) => ({ id: `out:${message.id}`, created: message.created_utc, message, reply: null })),
    ...replies.map((reply) => ({ id: `in:${reply.id}`, created: reply.created_utc, message: null, reply })),
  ].sort((a, b) => a.created.localeCompare(b.created) || a.id.localeCompare(b.id));
  useEffect(() => { followTimeline.current = true; }, [record.targetId]);
  const latest = timeline.at(-1);
  useEffect(() => {
    if (timelineRef.current && followTimeline.current)
      timelineRef.current.scrollTop = timelineRef.current.scrollHeight;
  }, [record.open, record.targetId, latest?.id, latest?.message?.state]);
  return (
    <aside className={`git-chat ${record.open ? "is-open" : ""}`} aria-label={`${projectName} operator chat`} onPointerDown={(event) => event.stopPropagation()}>
      <button className="git-chat-toggle" aria-expanded={record.open} aria-controls={record.open ? panelId : undefined} onClick={() => {
        const next = !record.open;
        update((prior) => ({ ...prior, open: next }));
        onToggle(next);
      }}>
        <span aria-hidden="true">◌</span>
        <span>{projectName} · Chat</span>
        <span aria-hidden="true">{record.open ? "−" : "+"}</span>
      </button>
      {record.open && <div id={panelId} className="git-chat-panel">
        <div className="git-chat-destination">
          <label>Send to
            <select aria-label={`${projectName} chat destination`} value={record.targetId} disabled={!snapshot?.targets.length || !!sendingId} onChange={(event) => update((prior) => ({ ...prior, targetId: event.target.value }))}>
              {!snapshot?.targets.length && <option value="">No destination available</option>}
              {snapshot?.targets.map((item) => <option key={item.id} value={item.id}>{item.role === "admiral" ? "Admiral" : "Captain"} · {item.label}</option>)}
            </select>
          </label>
          {target && <small title={target.id}>Exact target: <code>{target.id}</code></small>}
          {snapshot?.simulated && <strong className="git-chat-simulation">SIMULATED CHAT</strong>}
        </div>
        {sendBlock && <div className="git-chat-notice" role="status">{sendBlock}</div>}
        {snapshot?.history_truncated && <p className="git-chat-notice">Recent history window: older messages or replies were omitted.</p>}
        {(error || snapshot && !snapshot.enabled) && <button type="button" className="lv-text-button git-chat-access" onClick={onAccess}>Session access / setup</button>}
        <div ref={timelineRef} className="git-chat-timeline" role="log" aria-label={`${projectName} conversation`} aria-live="polite" aria-relevant="additions text" onScroll={(event) => {
          const element = event.currentTarget;
          followTimeline.current = element.scrollHeight - element.clientHeight - element.scrollTop < 24;
        }}>
          {!timeline.length && <p>Messages stay scoped to this project and exact destination.</p>}
          {timeline.map(({ id, message, reply }) => <article key={id} className={reply ? "git-chat-incoming" : "git-chat-outgoing"}>
            <b>{reply ? `${target?.role === "admiral" ? "Admiral" : "Captain"} reply` : message?.kind === "command" ? "Your command" : message?.kind === "git_health" ? "Git protector" : "Your message"}</b>
            <p>{reply?.text ?? message?.text}</p>
            <small>{reply ? "Reply received" : sendingId === message?.id ? "Sending…" : message?.state === "delivered" ? "Delivered to inbox; awaiting reply" : message?.state === "uncertain" ? "Delivery uncertain" : message?.state === "queued" ? "Queued; delivery pending" : "Delivery failed"}</small>
            {message?.detail && <small>{message.detail}</small>}
            {message?.state === "uncertain" && <button type="button" className="lv-text-button" disabled={!!sendingId || !!sendBlock} onClick={() => void send(retryChatRequest(message), false)}>Retry same message</button>}
            <time dateTime={reply?.created_utc ?? message?.created_utc}>{new Date(reply?.created_utc ?? message!.created_utc).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</time>
          </article>)}
        </div>
        <form onSubmit={(event) => {
          event.preventDefault();
          if (!target || !record.draft.trim() || bytes > CHAT_TEXT_BYTES) return;
          void send({ id: crypto.randomUUID(), project_id: projectId, target_id: target.id, kind: record.kind, text: record.draft.trim() }, true);
        }}>
          <div className="git-chat-compose-head">
            <label>Kind <select aria-label={`${projectName} chat kind`} value={record.kind} onChange={(event) => update((prior) => ({ ...prior, kind: event.target.value as "message" | "command" }))}>
              <option value="message">Message</option><option value="command">Command</option>
            </select></label>
            <small className={bytes > CHAT_TEXT_BYTES ? "over-limit" : ""}>{bytes} / {CHAT_TEXT_BYTES} bytes</small>
          </div>
          <label className="git-chat-compose-label" htmlFor={`${panelId}-text`}>{record.kind === "command" ? "Instruction for selected agent" : "Message for selected agent"}</label>
          <textarea id={`${panelId}-text`} aria-label={`${projectName} chat text`} rows={2} value={record.draft} placeholder={record.kind === "command" ? "Describe the action and project scope…" : "Flag an issue or ask a question…"} onChange={(event) => update((prior) => ({ ...prior, draft: event.target.value }))} />
          <div className="git-chat-compose-foot"><small>Commands are agent instructions. Delivery confirms inbox receipt.</small><button type="submit" className="lv-button" disabled={!!sendBlock || !!sendingId || !record.draft.trim() || bytes > CHAT_TEXT_BYTES}>Send {record.kind}</button></div>
        </form>
      </div>}
    </aside>
  );
}
