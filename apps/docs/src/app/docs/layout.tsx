import type { ReactNode } from "react";
import { DocsLayout } from "fumadocs-ui/layouts/docs";

import { baseOptions } from "@/app/layout.config";
import { V3BetaModal } from "@/components/V3BetaModal";
import { source } from "@/lib/source";

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <DocsLayout tree={source.pageTree} {...baseOptions} githubUrl="https://github.com/kyantech/Palmr" links={[]}>
      {children}
      <V3BetaModal />
    </DocsLayout>
  );
}
