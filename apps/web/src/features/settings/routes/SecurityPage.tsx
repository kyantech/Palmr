import { Divider } from "antd";
import { Children, Fragment, type ReactNode } from "react";
import { useEffectiveSettings } from "../api/queries";
import { PasswordSection } from "../components/PasswordForm";

export interface SecurityPageProps {
  canChangePassword: boolean;
  hasLocalPassword: boolean;
  children?: ReactNode;
}

export function SecurityPage({ canChangePassword, hasLocalPassword, children }: SecurityPageProps) {
  const settings = useEffectiveSettings();
  return (
    <>
      <PasswordSection
        canChangePassword={canChangePassword}
        hasLocalPassword={hasLocalPassword}
        passwordMinLength={settings.data?.passwordMinLength}
      />
      {Children.toArray(children).map((section, index) => (
        <Fragment key={index}>
          <Divider style={{ marginBlock: 40 }} />
          {section}
        </Fragment>
      ))}
    </>
  );
}
