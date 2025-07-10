import { createFromSource } from "fumadocs-core/search/server";

import { source } from "@/lib/source";

export const { GET } = createFromSource(source, (page) => {
  return {
    title: page.data.title,
    description: page.data.description,
    url: page.url,
    id: page.url,
    structuredData: page.data.structuredData,
    tag: page.url.startsWith("/docs/3.1-beta") ? "v3.1-beta" : "v2.0.0-beta",
  };
});
