import { Input, type InputRef, theme } from "antd";
import type { ChangeEvent, FocusEvent, Ref } from "react";
import type { FieldControlProps } from "../../../shared/ui/FormField";
import { BACKUP_CODE_DISPLAY_LENGTH, TOTP_CODE_LENGTH } from "./codeFormat";

export type CodeKind = "totp" | "backup";

interface OneTimeCodeInputProps extends FieldControlProps {
  kind: CodeKind;
  value: string;
  name: string;
  ref?: Ref<InputRef>;
  onChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onBlur: (event: FocusEvent<HTMLInputElement>) => void;
  readOnly?: boolean;
  size?: "middle" | "large";
}

export function OneTimeCodeInput({
  kind,
  size = "large",
  readOnly = false,
  ...props
}: OneTimeCodeInputProps) {
  const { token } = theme.useToken();
  const totp = kind === "totp";
  return (
    <Input
      {...props}
      size={size}
      readOnly={readOnly}
      autoComplete={totp ? "one-time-code" : "off"}
      inputMode={totp ? "numeric" : "text"}
      autoCapitalize={totp ? "none" : "characters"}
      spellCheck={false}
      maxLength={totp ? TOTP_CODE_LENGTH + 2 : BACKUP_CODE_DISPLAY_LENGTH + 4}
      style={{
        fontFamily: token.fontFamilyCode,
        letterSpacing: totp ? "0.3em" : "0.06em",
        fontVariantNumeric: "tabular-nums",
      }}
    />
  );
}
