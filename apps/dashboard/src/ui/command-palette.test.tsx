import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { CommandPalette, rankCommands, type PaletteCommand } from './CommandPalette';

const items: PaletteCommand[] = [
  { id: 'go:detail', label: 'Fleet detail', keywords: 'Projects, history, hierarchy', group: 'Go to', run: () => undefined },
  { id: 'go:health', label: 'Fleet health', keywords: 'Repository hygiene and the agent brief', group: 'Go to', run: () => undefined },
  { id: 'go:git', label: 'Git Flow', keywords: 'Branches and worktrees', group: 'Go to', run: () => undefined },
  { id: 'theme', label: 'Switch to light theme', keywords: 'Remembered in this browser', group: 'Actions', run: () => undefined },
];

describe('command palette', () => {
  it('matches every query word in label or hint and ranks label prefixes first', () => {
    expect(rankCommands(items, '').map(i => i.id)).toEqual(items.map(i => i.id));
    expect(rankCommands(items, 'git').map(i => i.id)).toEqual(['go:git']);
    expect(rankCommands(items, 'fleet hyg').map(i => i.id)).toEqual(['go:health']);
    expect(rankCommands(items, 'BRIEF').map(i => i.id)).toEqual(['go:health']);
    expect(rankCommands(items, 'worktrees').map(i => i.id)).toEqual(['go:git']);
    expect(rankCommands(items, 'nothing-here')).toEqual([]);
  });
  it('renders a labelled combobox and listbox with the first option active', () => {
    const html = renderToStaticMarkup(<CommandPalette commands={items} onClose={() => undefined} />);
    expect(html).toContain('aria-label="Command"');
    expect(html).toContain('role="combobox"');
    expect(html).toContain('role="listbox"');
    expect(html).toMatch(/role="option" aria-selected="true"[^>]*><span>Fleet detail</);
    expect(html).toContain('Arrow keys to move, <kbd>Enter</kbd> to open, <kbd>Esc</kbd> to close');
  });
});
