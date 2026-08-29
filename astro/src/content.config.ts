import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';

// Keep the repository's Markdown files as the single source of truth. The
// Astro app only supplies navigation and presentation for these documents.
const docs = defineCollection({
  loader: glob({ pattern: '**/*.md', base: '../docs' }),
});

export const collections = { docs };
