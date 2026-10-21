import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  // Dev server forwards every Rust API and probe path to a local admiraltyd.
  server: { proxy: Object.fromEntries(['/v1', '/v2', '/healthz', '/readyz', '/metrics', '/legacy'].map(path => [path, 'http://127.0.0.1:18891'])) },
  test: { environment: 'node' },
});
