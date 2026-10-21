import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import CommandPalette, { filterPalette, type PaletteItem } from './CommandPalette';

const items: PaletteItem[] = [
  { id: 'go:detail', label: 'Fleet detail', hint: 'Projects, history, hierarchy', group: 'Go to', href: '#/detail' },
  { id: 'go:health', label: 'Fleet health', hint: 'Repository hygiene and the agent brief', group: 'Go to', href: '#/health' },
  { id: 'go:git', label: 'Git Flow', hint: 'Branches and worktrees', group: 'Go to', href: '#/git' },
  { id: 'theme', label: 'Switch to light theme', hint: 'Remembered in this browser', group: 'Actions' },
];

describe('command palette', () => {
  it('matches every query word in label or hint and ranks label prefixes first', () => {
    expect(filterPalette(items, '').map(i => i.id)).toEqual(items.map(i => i.id));
    expect(filterPalette(items, 'git').map(i => i.id)).toEqual(['go:git']);
    expect(filterPalette(items, 'fleet hyg').map(i => i.id)).toEqual(['go:health']);
    expect(filterPalette(items, 'BRIEF').map(i => i.id)).toEqual(['go:health']);
    expect(filterPalette(items, 'worktrees').map(i => i.id)).toEqual(['go:git']);
    expect(filterPalette(items, 'nothing-here')).toEqual([]);
  });
  it('renders a labelled combobox and listbox with the first option active', () => {
    const html = renderToStaticMarkup(<CommandPalette items={items} onClose={() => undefined} />);
    expect(html).toContain('aria-label="Go to a view or run an action"');
    expect(html).toContain('role="combobox"');
    expect(html).toContain('role="listbox"');
    expect(html).toMatch(/role="option" aria-selected="true"[^>]*><span class="palette-option-label">Fleet detail</);
    expect(html).toContain('Arrow keys to move, Enter to open, Escape to close.');
  });
});
