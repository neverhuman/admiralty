import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  server: { proxy: { '/v1': 'http://127.0.0.1:18891', '/v2': 'http://127.0.0.1:18891' } },
  test: { environment: 'node' },
});
