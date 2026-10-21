import { useEffect, useId, useMemo, useRef, useState } from 'react';

export type PaletteItem = { id: string; label: string; hint: string; group: string; href?: string; run?: () => void };

/** Case-insensitive match on every word of the query, in label or hint. */
export function filterPalette(items: PaletteItem[], query: string): PaletteItem[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return items;
  return items
    .map(item => ({ item, text: `${item.label} ${item.hint}`.toLowerCase(), label: item.label.toLowerCase() }))
    .filter(entry => words.every(word => entry.text.includes(word)))
    .sort((a, b) => Number(!a.label.startsWith(words[0])) - Number(!b.label.startsWith(words[0])))
    .map(entry => entry.item);
}

/**
 * Keyboard-first destination and action picker (Ctrl/Cmd+K). A native modal
 * dialog provides focus containment and Escape; the input follows the ARIA
 * combobox pattern so the highlighted option is announced.
 */
export default function CommandPalette({ items, onClose }: { items: PaletteItem[]; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const listId = useId();
  const matches = useMemo(() => filterPalette(items, query), [items, query]);
  useEffect(() => {
    const element = dialog.current;
    if (element && !element.open && typeof element.showModal === 'function') element.showModal();
    return () => { if (element?.open) element.close(); };
  }, []);
  useEffect(() => setActive(0), [query]);
  const choose = (item: PaletteItem | undefined) => {
    if (!item) return;
    onClose();
    if (item.href) window.location.hash = item.href;
    item.run?.();
  };
  const optionId = (index: number) => `${listId}-option-${index}`;
  return <dialog ref={dialog} className="palette-dialog" aria-label="Go to a view or run an action" onClose={onClose}
    onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="palette">
      <label className="palette-label" htmlFor={`${listId}-input`}>Go to</label>
      <input id={`${listId}-input`} className="palette-input" autoFocus autoComplete="off" spellCheck={false}
        role="combobox" aria-expanded="true" aria-controls={listId} aria-autocomplete="list"
        aria-activedescendant={matches.length ? optionId(Math.min(active, matches.length - 1)) : undefined}
        placeholder="Type a view or action" value={query} onChange={event => setQuery(event.target.value)}
        onKeyDown={event => {
          if (event.key === 'ArrowDown') { event.preventDefault(); setActive(index => Math.min(matches.length - 1, index + 1)); }
          else if (event.key === 'ArrowUp') { event.preventDefault(); setActive(index => Math.max(0, index - 1)); }
          else if (event.key === 'Enter') { event.preventDefault(); choose(matches[Math.min(active, matches.length - 1)]); }
        }} />
      <ul id={listId} role="listbox" className="palette-list" aria-label="Matching views and actions">
        {matches.map((item, index) => <li key={item.id} id={optionId(index)} role="option" aria-selected={index === active}
          className={`palette-option ${index === active ? 'active' : ''}`}
          onMouseEnter={() => setActive(index)} onMouseDown={event => { event.preventDefault(); choose(item); }}>
          <span className="palette-option-label">{item.label}</span><span className="palette-option-hint">{item.hint}</span><span className="palette-option-group">{item.group}</span>
        </li>)}
        {!matches.length && <li className="palette-empty" role="option" aria-selected="false" aria-disabled="true">Nothing matches “{query}”. Try a view name such as health or git.</li>}
      </ul>
      <p className="palette-help">Arrow keys to move, Enter to open, Escape to close.</p>
    </div>
  </dialog>;
}
