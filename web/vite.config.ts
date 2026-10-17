import { defineConfig } from 'vite';

// In dev, Vite serves the client and forwards API + WebSocket traffic to the Rust server.
// In production, Caddy does the same job.
export default defineConfig({
  server: {
    host: true, // reachable from your phone on the same Wi-Fi
    port: 5180,
    strictPort: true,
    proxy: {
      '/api': 'http://localhost:3000',
      '/ws': { target: 'ws://localhost:3000', ws: true },
    },
  },
});
