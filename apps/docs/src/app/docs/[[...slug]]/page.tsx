import { redirect } from "next/navigation";
import { createRelativeLink } from "fumadocs-ui/mdx";
import { DocsBody, DocsDescription, DocsPage, DocsTitle } from "fumadocs-ui/page";

import { source } from "@/lib/source";
import { getMDXComponents } from "@/mdx-components";
import { Footer } from "../components/footer";
import { Sponsor } from "../components/sponsor";

export default async function Page(props: { params: Promise<{ slug?: string[] }> }) {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) redirect("/docs/v3-beta");

  const MDXContent = page.data.body;

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
  if (!page) redirect("/docs/v3-beta");

  return {
    title: page.data.title + " | Palmr. Docs",
    description: page.data.description,
  };
}
