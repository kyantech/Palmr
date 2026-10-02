import { theme, Typography } from "antd";
import type { ReactNode } from "react";

export interface Fact {
  key: string;
  label: string;
  value: ReactNode;
}

export function FactList({ facts }: { facts: readonly Fact[] }) {
  const { token } = theme.useToken();
  return (
    <dl
      style={{
        margin: 0,
        display: "grid",
        gridTemplateColumns: "minmax(120px, 200px) minmax(0, 1fr)",
        columnGap: token.margin,
        rowGap: token.marginXS,
        alignItems: "baseline",
      }}
    >
      {facts.map((fact) => (
        <FactRow key={fact.key} fact={fact} />
      ))}
    </dl>
  );
}

function FactRow({ fact }: { fact: Fact }) {
  return (
    <>
      <dt>
        <Typography.Text type="secondary">{fact.label}</Typography.Text>
      </dt>
      <dd style={{ margin: 0, minWidth: 0, overflowWrap: "anywhere" }} data-fact={fact.key}>
        {fact.value}
      </dd>
    </>
  );
}
