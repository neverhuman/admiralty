import { useEffect, useId, useMemo, useRef, useState } from 'react';

export type PaletteCommand = { id: string; label: string; group: string; keywords?: string; run: () => void };

/** Rank commands for a query: label prefix, then word prefix, then substring. */
export function rankCommands(commands: PaletteCommand[], query: string): PaletteCommand[] {
  const q = query.trim().toLowerCase();
  if (!q) return commands;
  const score = (command: PaletteCommand) => {
    const label = command.label.toLowerCase(), haystack = `${label} ${command.group.toLowerCase()} ${command.keywords?.toLowerCase() ?? ''}`;
    if (label.startsWith(q)) return 0;
    if (label.split(/\s+/).some(word => word.startsWith(q))) return 1;
    if (haystack.includes(q)) return 2;
    return -1;
  };
  return commands.map(command => ({ command, rank: score(command) }))
    .filter(item => item.rank >= 0)
    .sort((a, b) => a.rank - b.rank)
    .map(item => item.command);
}

/**
 * Keyboard-first navigation (Ctrl/⌘ K). A combobox over a listbox: arrows move,
 * Enter runs, Escape closes and focus returns to where it was.
 */
export function CommandPalette({ commands, onClose }: { commands: PaletteCommand[]; onClose: () => void }) {
  const [query, setQuery] = useState(''), [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null), returnFocus = useRef<Element | null>(null);
  const listId = useId();
  const results = useMemo(() => rankCommands(commands, query), [commands, query]);
  useEffect(() => {
    returnFocus.current = document.activeElement;
    input.current?.focus();
    return () => { if (returnFocus.current instanceof HTMLElement) returnFocus.current.focus(); };
  }, []);
  useEffect(() => setActive(0), [query]);
  const run = (command: PaletteCommand | undefined) => { if (!command) return; onClose(); command.run(); };
  return <div className="palette-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="palette" role="dialog" aria-modal="true" aria-labelledby={`${listId}-title`}>
      <h2 id={`${listId}-title`} className="palette-title">Go to a view or run a command</h2>
      <input ref={input} className="palette-input" value={query} placeholder="Go to a view or run a command…" aria-label="Command"
        role="combobox" aria-expanded="true" aria-controls={listId} aria-autocomplete="list"
        aria-activedescendant={results[active] ? `${listId}-option-${active}` : undefined}
        onChange={event => setQuery(event.target.value)}
        onKeyDown={event => {
          if (event.key === 'Escape') { event.preventDefault(); onClose(); }
          else if (event.key === 'ArrowDown') { event.preventDefault(); setActive(index => Math.min(results.length - 1, index + 1)); }
          else if (event.key === 'ArrowUp') { event.preventDefault(); setActive(index => Math.max(0, index - 1)); }
          else if (event.key === 'Enter') { event.preventDefault(); run(results[active]); }
        }} />
      <ul id={listId} className="palette-list" role="listbox" aria-label="Commands">
        {results.map((command, index) => <li key={command.id} id={`${listId}-option-${index}`} role="option" aria-selected={index === active}
          className={index === active ? 'active' : ''} onMouseEnter={() => setActive(index)} onMouseDown={event => { event.preventDefault(); run(command); }}>
          <span>{command.label}</span><small>{command.group}</small>
        </li>)}
        {!results.length && <li className="palette-empty" role="option" aria-selected="false" aria-disabled="true">No matching view or command</li>}
      </ul>
      <p className="palette-hint">Arrow keys to move, <kbd>Enter</kbd> to open, <kbd>Esc</kbd> to close</p>
    </section>
  </div>;
}
