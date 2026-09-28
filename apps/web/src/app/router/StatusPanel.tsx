import { Button, Layout, Result, Typography } from "antd";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { PATHS } from "./paths";

interface StatusPanelProps {
  code: "403" | "404";
  title: string;
  description: string;
  action: string;
  destination: string;
}

function StatusPanel({ code, title, description, action, destination }: StatusPanelProps) {
  const navigate = useNavigate();
  const titleId = useId();
  return (
    <Layout style={{ minHeight: "100dvh", justifyContent: "center" }}>
      <Layout.Content style={{ flex: "none", padding: 24 }}>
        <section aria-labelledby={titleId} data-status={code}>
          <Result
            icon={
              <Typography.Text
                type="secondary"
                aria-hidden="true"
                style={{ fontSize: 56, fontWeight: 600, lineHeight: 1, letterSpacing: "-0.02em" }}
              >
                {code}
              </Typography.Text>
            }
            title={
              <h1 id={titleId} style={{ margin: 0, font: "inherit", color: "inherit" }}>
                {title}
              </h1>
            }
            subTitle={description}
            extra={
              <Button
                type="primary"
                onClick={() => {
                  void navigate(destination);
                }}
              >
                {action}
              </Button>
            }
          />
        </section>
      </Layout.Content>
    </Layout>
  );
}

export function NotFoundPanel() {
  const { t } = useTranslation("common");
  return (
    <StatusPanel
      code="404"
      title={t("status.notFound.title")}
      description={t("status.notFound.description")}
      action={t("status.notFound.action")}
      destination={PATHS.root}
    />
  );
}

export function ForbiddenPanel() {
  const { t } = useTranslation("common");
  return (
    <StatusPanel
      code="403"
      title={t("status.forbidden.title")}
      description={t("status.forbidden.description")}
      action={t("status.forbidden.action")}
      destination={PATHS.overview}
    />
  );
}
