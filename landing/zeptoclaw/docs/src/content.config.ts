import { z, defineCollection } from 'astro:content';
import { docsSchema } from '@astrojs/starlight/schema';

const docsCollection = defineCollection({
  schema: docsSchema(),
});

export const collections = {
  'docs': docsCollection,
};
