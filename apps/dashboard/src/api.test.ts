import { describe, expect, it } from 'vitest';
import { fleetTotals, formatUnknown, getNativeFleet, taskStateLabel, type Fleet } from './api';

const fleet = {
  schema_version: 1,
  simulated: true,
  cursor: { recovery_generation: 'generation', event_seq: '9007199254740993' },
  projects: [
    { id: 'a', name: 'Alpha', total: 8, running: 2, awaiting_verification: 1, blocked: 1, accepted: 3 },
    { id: 'b', name: 'Beta', total: 4, running: 0, awaiting_verification: 1, blocked: 0, accepted: 2 },
  ],
  tasks: [],
  coordination: { queued_messages: 2, acknowledged_messages: 4, routine_model_turns: 0, measured_tokens: null },
  sqlite_version: '3',
  native_execution_qualified: false,
} as Fleet;

describe('fleet display semantics', () => {
  it('fetches and validates the authenticated v2 fleet snapshot', async () => {
    const snapshot = { ...fleet, schema_version: 2 };
    let requested = '';
    const result = await getNativeFleet('session-token', undefined, async (input, init) => {
      requested = String(input);
      expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer session-token');
      return Response.json(snapshot);
    });
    expect(requested).toBe('/v2/fleet');
    expect(result.schema_version).toBe(2);
  });

  it('keeps unknown usage distinct from zero', () => {
    expect(formatUnknown(null)).toBe('Unknown');
    expect(formatUnknown(0)).toBe('0');
  });

  it('aggregates stages from project counts', () => {
    expect(fleetTotals(fleet)).toEqual({ projects: 2, tasks: 12, running: 2, blocked: 1, accepted: 5, verifying: 2 });
  });

  it('preserves large event cursors as strings and labels task states readably', () => {
    expect(fleet.cursor.event_seq).toBe('9007199254740993');
    expect(taskStateLabel('awaiting_verification')).toBe('awaiting verification');
  });
});
