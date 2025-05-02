import { source } from '@/lib/source';
import { createFromSource } from 'fumadocs-core/search/server';
 
export const { GET } = createFromSource(source, (page) => {
  return {
    title: page.data.title,
    description: page.data.description,
    url: page.url,
    id: page.url,
    structuredData: page.data.structuredData,
    tag: page.url.startsWith('/docs/2.0.0-beta') ? 'v2.0.0-beta' : 'v1.1.7-beta'
  };
});
