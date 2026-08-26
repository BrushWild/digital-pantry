import { defineConfig } from 'vite';
import path from 'path';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  base: '/digital-pantry/',
  resolve: {
    alias: { '@': path.resolve(__dirname, 'src') },
  },
  plugins: [react(), tailwindcss()],
  build: {
    outDir: 'dist',
  },
  server: { port: 5173 },
});
