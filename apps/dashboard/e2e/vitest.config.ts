import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    // Both suites qualify the same built dist directory; parallel rebuilds race.
    fileParallelism: false,
    include: ['e2e/**/*.e2e.ts'],
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});
