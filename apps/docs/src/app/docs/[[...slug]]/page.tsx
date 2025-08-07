import { redirect } from "next/navigation";
import { createRelativeLink } from "fumadocs-ui/mdx";
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from "fumadocs-ui/page";

import { VersionWarning } from "@/components/version-warning";
import { source } from "@/lib/source";
import { getMDXComponents } from "@/mdx-components";
import { Footer } from "../components/footer";
import { Sponsor } from "../components/sponsor";

export default async function Page(props: { params: Promise<{ slug?: string[] }> }) {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) redirect("/docs/3.1-beta");

  const MDXContent = page.data.body;

  const shouldShowWarning = page.url.startsWith("/docs/2.0.0-beta");

  return (
    <DocsPage
      toc={page.data.toc}
      full={page.data.full}
      footer={{ enabled: true, component: <Footer /> }}
      tableOfContent={{
        style: "clerk",
        footer: <Sponsor />,
      }}
    >
      {shouldShowWarning && <VersionWarning />}
      <DocsTitle>{page.data.title}</DocsTitle>
      <div className="border w-full"></div>
      <DocsDescription>{page.data.description}</DocsDescription>
      <DocsBody>
        <MDXContent
          components={getMDXComponents({
            a: createRelativeLink(source, page),
          })}
        />
      </DocsBody>
    </DocsPage>
  );
}

export async function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata(props: { params: Promise<{ slug?: string[] }> }) {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) redirect("/docs/3.1-beta");

  return {
    title: page.data.title + " | Palmr. Docs",
    description: page.data.description,
  };
}
