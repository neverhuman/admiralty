import { afterEach, describe, expect, it, vi } from 'vitest';
import { scheduleVisible, type VisibilityDocument } from './visibility';

function fakeDocument(initial: DocumentVisibilityState) {
  const listeners = new Set<() => void>();
  const doc = {
    visibilityState: initial,
    addEventListener: (_type: string, listener: () => void) => { listeners.add(listener); },
    removeEventListener: (_type: string, listener: () => void) => { listeners.delete(listener); },
  };
  return {
    doc: doc as unknown as VisibilityDocument,
    listeners,
    set(state: DocumentVisibilityState) { doc.visibilityState = state; for (const listener of [...listeners]) listener(); },
  };
}

describe('visibility-aware scheduling', () => {
  afterEach(() => vi.useRealTimers());
  it('runs after the delay while visible', async () => {
    vi.useFakeTimers();
    const task = vi.fn(), page = fakeDocument('visible');
    scheduleVisible(task, 15_000, page.doc);
    await vi.advanceTimersByTimeAsync(14_999); expect(task).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1); expect(task).toHaveBeenCalledTimes(1);
  });
  it('waits for a hidden page to become visible, then runs once immediately', async () => {
    vi.useFakeTimers();
    const task = vi.fn(), page = fakeDocument('hidden');
    scheduleVisible(task, 1_000, page.doc);
    await vi.advanceTimersByTimeAsync(60_000); expect(task).not.toHaveBeenCalled();
    page.set('hidden'); expect(task).not.toHaveBeenCalled();
    page.set('visible'); expect(task).toHaveBeenCalledTimes(1);
    page.set('hidden'); page.set('visible'); expect(task).toHaveBeenCalledTimes(1);
    expect(page.listeners.size).toBe(0);
  });
  it('cancel clears both the timer and a pending visibility listener', async () => {
    vi.useFakeTimers();
    const task = vi.fn(), page = fakeDocument('hidden');
    const cancel = scheduleVisible(task, 10, page.doc);
    await vi.advanceTimersByTimeAsync(10); expect(page.listeners.size).toBe(1);
    cancel(); expect(page.listeners.size).toBe(0);
    page.set('visible'); expect(task).not.toHaveBeenCalled();
    const second = vi.fn();
    scheduleVisible(second, 10, page.doc)();
    await vi.advanceTimersByTimeAsync(20); expect(second).not.toHaveBeenCalled();
  });
  it('treats environments without a document as visible', async () => {
    vi.useFakeTimers();
    const task = vi.fn();
    scheduleVisible(task, 5, undefined);
    await vi.advanceTimersByTimeAsync(5); expect(task).toHaveBeenCalledTimes(1);
  });
});
