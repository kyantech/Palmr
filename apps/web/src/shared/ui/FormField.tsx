import { Form } from "antd";
import type { CSSProperties, ReactNode } from "react";

export interface FieldControlProps {
  id: string;
  "aria-invalid": boolean;
  "aria-describedby"?: string;
}

interface FormFieldProps {
  id: string;
  label: ReactNode;
  error?: string | undefined;
  extra?: ReactNode;
  style?: CSSProperties;
  children: (control: FieldControlProps) => ReactNode;
}

export function FormField({ id, label, error, extra, style, children }: FormFieldProps) {
  const helpId = `${id}-help`;
  const extraId = `${id}-extra`;
  const describedBy = [error ? helpId : null, extra ? extraId : null].filter(Boolean).join(" ");
  return (
    <Form.Item
      label={label}
      htmlFor={id}
      {...(style === undefined ? {} : { style })}
      {...(error ? { validateStatus: "error", help: <span id={helpId}>{error}</span> } : {})}
      {...(extra ? { extra: <span id={extraId}>{extra}</span> } : {})}
    >
      {children({
        id,
        "aria-invalid": Boolean(error),
        ...(describedBy === "" ? {} : { "aria-describedby": describedBy }),
      })}
    </Form.Item>
  );
}
