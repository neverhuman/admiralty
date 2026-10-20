import { describe, expect, it, vi } from "vitest";
import {
  CHAT_TEXT_BYTES, createChatMemory, loadOperatorChat, mergeChatMessages,
  newProjectChat, parseChatSnapshot, postOperatorChat, retryChatRequest, textBytes,
  type ChatMessage, type ChatRequest,
} from "./operator-chat";

const request: ChatRequest = {
  id: "c088dace-717a-4d7d-8ca9-2c81a35b8001", project_id: "dope / exact", target_id: "captain:dope:1",
  kind: "command", text: "Inspect divergent refs; propose a reconciliation.",
};
const delivered: ChatMessage = { ...request, state: "delivered", created_utc: "2026-10-20T12:00:00Z", detail: "Delivered to target inbox; acknowledgment and execution are separate." };
function snapshot() {
  return {
    schema_version: 1, enabled: true, simulated: false,
    targets: [{ id: request.target_id, role: "captain", label: "DOPE captain" }],
    messages: [delivered], responses: [{ id: "reply:1", project_id: request.project_id, target_id: request.target_id, text: "Inspecting; no refs changed.", created_utc: "2026-10-20T12:00:01Z" }], error: null,
  };
}
describe("project operator chat transport", () => {
  it("routes exact project, target and token, and validates delivery identity", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(Response.json(snapshot()));
    const signal = new AbortController().signal;
    expect((await loadOperatorChat(request.project_id, "fixture-token", signal, fetcher)).responses[0].text).toContain("no refs changed");
    expect(fetcher).toHaveBeenCalledWith("/v2/operator-chat?project_id=dope%20%2F%20exact", expect.objectContaining({ signal, cache: "no-store", headers: expect.objectContaining({ Authorization: "Bearer fixture-token" }) }));
    fetcher.mockResolvedValueOnce(Response.json(delivered));
    expect(await postOperatorChat(request, "fixture-token", signal, fetcher)).toEqual(delivered);
    const posted = fetcher.mock.calls[1];
    expect(posted[0]).toBe("/v2/operator-chat");
    expect(JSON.parse(String(posted[1]?.body))).toEqual(request);
    fetcher.mockResolvedValueOnce(Response.json({ ...delivered, target_id: "admiral:other" }));
    await expect(postOperatorChat(request, undefined, signal, fetcher)).rejects.toThrow("identity mismatch");
  });
  it("rejects cross-project history/replies, duplicate targets and unsupported versions", () => {
    expect(() => parseChatSnapshot(snapshot(), "other-project")).toThrow("project mismatch");
    expect(() => parseChatSnapshot({ ...snapshot(), responses: [{ ...snapshot().responses[0], project_id: "other" }] }, request.project_id)).toThrow("project mismatch");
    expect(() => parseChatSnapshot({ ...snapshot(), targets: [...snapshot().targets, ...snapshot().targets] }, request.project_id)).toThrow("Ambiguous");
    expect(() => parseChatSnapshot({ ...snapshot(), schema_version: 2 }, request.project_id)).toThrow("Unsupported");
    expect(() => parseChatSnapshot({ ...snapshot(), enabled: false }, request.project_id)).toThrow("Ambiguous");
    expect(parseChatSnapshot({ ...snapshot(), history_truncated: true }, request.project_id).history_truncated).toBe(true);
    expect(() => parseChatSnapshot({ ...snapshot(), history_truncated: "false" }, request.project_id)).toThrow("Unsupported");
  });
  it("retries uncertain delivery with the original immutable UUID and payload", async () => {
    const uncertain: ChatMessage = { ...delivered, state: "uncertain", detail: "Response lost" };
    const retry = retryChatRequest(uncertain);
    expect(retry).toEqual(request);
    expect(retry.id).toBe(uncertain.id);
    const fetcher = vi.fn<typeof fetch>().mockRejectedValueOnce(new TypeError("response lost")).mockResolvedValueOnce(Response.json(delivered));
    const signal = new AbortController().signal;
    await expect(postOperatorChat(request, undefined, signal, fetcher)).rejects.toThrow("response lost");
    await postOperatorChat(retry, undefined, signal, fetcher);
    expect(fetcher.mock.calls[0][1]?.body).toBe(fetcher.mock.calls[1][1]?.body);
    expect(mergeChatMessages([delivered], [uncertain])).toEqual([delivered]);
    expect(() => retryChatRequest(delivered)).toThrow("cannot be retried");
  });
  it("enforces UTF-8 byte limits before network access", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(Response.json({ ...delivered, text: "é".repeat(CHAT_TEXT_BYTES / 2) }));
    const signal = new AbortController().signal;
    const exact = "é".repeat(CHAT_TEXT_BYTES / 2);
    expect(textBytes(exact)).toBe(CHAT_TEXT_BYTES);
    await postOperatorChat({ ...request, text: exact }, undefined, signal, fetcher);
    await expect(postOperatorChat({ ...request, text: `${exact}é` }, undefined, signal, fetcher)).rejects.toThrow("8192");
    await expect(postOperatorChat({ ...request, text: "   " }, undefined, signal, fetcher)).rejects.toThrow("8192");
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
  it("retains project drafts only within the current access scope", () => {
    const scope = createChatMemory();
    scope.projects.set("dope", { ...newProjectChat(), draft: "private draft", targetId: "captain:dope", messages: [delivered] });
    scope.projects.set("other", { ...newProjectChat(), draft: "other draft" });
    expect(scope.projects.get("dope")?.draft).toBe("private draft");
    expect(scope.projects.get("other")?.draft).toBe("other draft");
    const changedAccess = createChatMemory();
    expect(changedAccess.id).not.toBe(scope.id);
    expect(changedAccess.projects.size).toBe(0);
  });
});
