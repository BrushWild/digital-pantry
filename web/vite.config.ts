import { defineConfig } from 'vite';

export default defineConfig({
  base: '/digital-pantry/',
  build: {
    outDir: 'dist',
  },
  server: { port: 5173 },
});
