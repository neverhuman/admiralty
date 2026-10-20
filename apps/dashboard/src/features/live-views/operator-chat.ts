export const CHAT_TEXT_BYTES = 8 * 1024;
const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;

export type ChatTarget = { id: string; role: "admiral" | "captain"; label: string };
export type ChatMessage = {
  id: string;
  project_id: string;
  target_id: string;
  kind: "message" | "command" | "git_health";
  text: string;
  state: "queued" | "delivered" | "uncertain" | "failed";
  created_utc: string;
  detail: string | null;
};
export type ChatReply = {
  id: string;
  project_id: string;
  target_id: string;
  text: string;
  created_utc: string;
};
export type ChatSnapshot = {
  schema_version: 1;
  enabled: boolean;
  simulated: boolean;
  targets: ChatTarget[];
  messages: ChatMessage[];
  responses: ChatReply[];
  history_truncated: boolean;
  error: string | null;
};
export type ChatRequest = Pick<ChatMessage, "id" | "project_id" | "target_id" | "text"> & {
  kind: "message" | "command";
};
export type ChatProjectMemory = {
  open: boolean;
  draft: string;
  kind: "message" | "command";
  targetId: string;
  messages: ChatMessage[];
  replies: ChatReply[];
};
let memorySerial = 0;
export function createChatMemory() {
  return { id: ++memorySerial, projects: new Map<string, ChatProjectMemory>() };
}
export type ChatMemory = ReturnType<typeof createChatMemory>;
export function newProjectChat(): ChatProjectMemory {
  return { open: false, draft: "", kind: "message", targetId: "", messages: [], replies: [] };
}
export function textBytes(text: string) {
  return new TextEncoder().encode(text).length;
}
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Unsupported operator chat response");
  return value as Record<string, unknown>;
}
function field(value: unknown, maxBytes = CHAT_TEXT_BYTES): string {
  if (typeof value !== "string" || !value || textBytes(value) > maxBytes)
    throw new Error("Invalid operator chat field");
  return value;
}
function timestamp(value: unknown): string {
  const stamp = field(value, 128);
  if (!Number.isFinite(Date.parse(stamp))) throw new Error("Invalid operator chat time");
  return stamp;
}
function identity(value: unknown, projectId: string) {
  const item = object(value);
  if (field(item.project_id, 512) !== projectId)
    throw new Error("Operator chat project mismatch");
  return {
    id: field(item.id, 512),
    project_id: projectId,
    target_id: field(item.target_id, 512),
    text: field(item.text),
    created_utc: timestamp(item.created_utc),
  };
}
export function parseChatMessage(value: unknown, projectId: string): ChatMessage {
  const item = object(value), common = identity(value, projectId);
  if (!["message", "command", "git_health"].includes(String(item.kind)) ||
      !["queued", "delivered", "uncertain", "failed"].includes(String(item.state)) ||
      !(item.detail === null || typeof item.detail === "string" && textBytes(item.detail) <= CHAT_TEXT_BYTES))
    throw new Error("Invalid operator chat delivery");
  return { ...common, kind: item.kind as ChatMessage["kind"], state: item.state as ChatMessage["state"], detail: item.detail as string | null };
}
export function parseChatSnapshot(value: unknown, projectId: string): ChatSnapshot {
  const item = object(value);
  if (item.schema_version !== 1 || typeof item.enabled !== "boolean" || typeof item.simulated !== "boolean" ||
      !Array.isArray(item.targets) || item.targets.length > 32 ||
      !Array.isArray(item.messages) || item.messages.length > 500 ||
      !Array.isArray(item.responses) || item.responses.length > 500 ||
      !(item.history_truncated === undefined || typeof item.history_truncated === "boolean") ||
      !(item.error === null || typeof item.error === "string" && textBytes(item.error) <= CHAT_TEXT_BYTES))
    throw new Error("Unsupported operator chat snapshot");
  const targets = item.targets.map((value): ChatTarget => {
    const target = object(value);
    if (target.role !== "admiral" && target.role !== "captain")
      throw new Error("Unsupported operator chat target");
    return { id: field(target.id, 512), role: target.role, label: field(target.label, 512) };
  });
  if (new Set(targets.map((target) => target.id)).size !== targets.length || !item.enabled && targets.length)
    throw new Error("Ambiguous operator chat target");
  const messages = item.messages.map((message) => parseChatMessage(message, projectId));
  const responses = item.responses.map((reply) => identity(reply, projectId));
  if (new Set(messages.map((message) => message.id)).size !== messages.length ||
      new Set(responses.map((reply) => reply.id)).size !== responses.length)
    throw new Error("Duplicate operator chat identity");
  return { schema_version: 1, enabled: item.enabled, simulated: item.simulated, targets, messages, responses, history_truncated: item.history_truncated === true, error: item.error as string | null };
}
export function mergeChatMessages(remote: ChatMessage[], local: ChatMessage[]) {
  const messages = new Map(local.map((message) => [message.id, message]));
  for (const message of remote) messages.set(message.id, message);
  return [...messages.values()].sort((a, b) => a.created_utc.localeCompare(b.created_utc) || a.id.localeCompare(b.id)).slice(-500);
}
export function retryChatRequest(message: ChatMessage): ChatRequest {
  if (!["uncertain", "failed"].includes(message.state) || message.kind === "git_health")
    throw new Error("This delivery cannot be retried");
  return { id: message.id, project_id: message.project_id, target_id: message.target_id, kind: message.kind, text: message.text };
}
export class ChatApiError extends Error {
  constructor(readonly status: number) {
    super([401, 403].includes(status) ? "Session access required for operator chat" : `Operator chat request failed (${status})`);
  }
}
export async function readBoundedJson(response: Response, maxBytes = MAX_RESPONSE_BYTES): Promise<unknown> {
  if (!response.ok) throw new ChatApiError(response.status);
  if (!response.body) throw new Error("Operator chat response missing");
  const reader = response.body.getReader(), decoder = new TextDecoder("utf-8", { fatal: true });
  let bytes = 0, text = "";
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      bytes += chunk.value.byteLength;
      if (bytes > maxBytes) throw new Error("API response too large");
      text += decoder.decode(chunk.value, { stream: true });
    }
    return JSON.parse(text + decoder.decode());
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}
function headers(token?: string): Record<string, string> {
  return { Accept: "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) };
}
export async function loadOperatorChat(projectId: string, token: string | undefined, signal: AbortSignal, fetcher = fetch) {
  const response = await fetcher(`/v2/operator-chat?project_id=${encodeURIComponent(projectId)}`, {
    signal, cache: "no-store", headers: headers(token),
  });
  return parseChatSnapshot(await readBoundedJson(response), projectId);
}
export async function postOperatorChat(request: ChatRequest, token: string | undefined, signal: AbortSignal, fetcher = fetch) {
  if (!request.text.trim() || textBytes(request.text) > CHAT_TEXT_BYTES ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(request.id))
    throw new Error("Operator chat message must have a UUID and 1–8192 bytes of text");
  const response = await fetcher("/v2/operator-chat", {
    method: "POST", signal, cache: "no-store", headers: { ...headers(token), "Content-Type": "application/json" }, body: JSON.stringify(request),
  });
  const message = parseChatMessage(await readBoundedJson(response), request.project_id);
  if (message.id !== request.id || message.target_id !== request.target_id || message.kind !== request.kind || message.text !== request.text)
    throw new Error("Operator chat delivery identity mismatch");
  return message;
}
