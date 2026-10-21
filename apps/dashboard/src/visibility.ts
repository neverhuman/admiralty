/** Minimal document surface used for page-visibility scheduling. */
export type VisibilityDocument = Pick<Document, 'visibilityState' | 'addEventListener' | 'removeEventListener'>;

const currentDocument = (): VisibilityDocument | undefined => (typeof document === 'undefined' ? undefined : document);

/**
 * Run `task` after `delayMs`. If the page is hidden when the delay ends, wait
 * until it is visible again and then run immediately, so background tabs do
 * not keep downloading and parsing multi-megabyte snapshots. Returns a cancel
 * function that clears both the timer and any visibility listener.
 */
export function scheduleVisible(task: () => void, delayMs: number, doc: VisibilityDocument | undefined = currentDocument()): () => void {
  let listener: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined = setTimeout(() => {
    timer = undefined;
    if (!doc || doc.visibilityState !== 'hidden') {
      task();
      return;
    }
    listener = () => {
      if (doc.visibilityState === 'hidden') return;
      doc.removeEventListener('visibilitychange', listener!);
      listener = undefined;
      task();
    };
    doc.addEventListener('visibilitychange', listener);
  }, delayMs);
  return () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    if (listener && doc) doc.removeEventListener('visibilitychange', listener);
    listener = undefined;
  };
}
