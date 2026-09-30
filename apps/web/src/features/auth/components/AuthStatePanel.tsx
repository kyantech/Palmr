import { Flex, theme } from "antd";
import type { ReactNode } from "react";
import { ErrorTechnicalDetails, presentError, useErrorMessage } from "../../../shared/errors";
import { AuthHeading } from "../../../shared/ui/AuthHeading";

interface AuthStatePanelProps {
  title: string;
  error: unknown;
  actions: ReactNode;
  testId: string;
}

export function AuthStatePanel({ title, error, actions, testId }: AuthStatePanelProps) {
  const { token } = theme.useToken();
  const description = useErrorMessage(error);
  const presented = presentError(error);
  return (
    <div data-testid={testId} data-error-code={presented.code ?? undefined}>
      <AuthHeading title={title} description={description} />
      <Flex vertical gap={token.marginSM}>
        <ErrorTechnicalDetails presented={presented} />
        {actions}
      </Flex>
    </div>
  );
}
