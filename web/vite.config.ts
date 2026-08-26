import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  base: '/digital-pantry/',
  plugins: [react(), tailwindcss()],
  build: {
    outDir: 'dist',
  },
  server: { port: 5173 },
});
