import { Flex, InputNumber, Select, Space, Switch } from "antd";
import { useTranslation } from "react-i18next";
import { type ByteUnit, MAX_SAFE_BYTES, toBytes } from "../../../shared/format/bytes";
import type { FieldControlProps } from "../../../shared/ui/FormField";

export const BYTE_UNITS: readonly ByteUnit[] = ["KiB", "MiB", "GiB", "TiB"];

export interface BytesValue {
  amount: number | null;
  unit: ByteUnit;
}

interface BytesInputProps extends FieldControlProps {
  value: BytesValue;
  onChange: (value: BytesValue) => void;
  disabled?: boolean;
}

export function BytesInput({ value, onChange, disabled = false, ...control }: BytesInputProps) {
  const { t } = useTranslation("admin");
  const units: readonly ByteUnit[] = BYTE_UNITS.includes(value.unit)
    ? BYTE_UNITS
    : [value.unit, ...BYTE_UNITS];
  return (
    <Space.Compact style={{ width: "100%" }}>
      <InputNumber
        {...control}
        min={0}
        value={value.amount}
        disabled={disabled}
        style={{ flex: 1 }}
        onChange={(amount) => {
          onChange({ ...value, amount });
        }}
      />
      <Select<ByteUnit>
        aria-label={t("bytes.unit")}
        value={value.unit}
        disabled={disabled}
        style={{ width: 96 }}
        options={units.map((unit) => ({ value: unit, label: unit }))}
        onChange={(unit) => {
          onChange({ ...value, unit });
        }}
      />
    </Space.Compact>
  );
}

export function bytesOf(value: BytesValue): number | null {
  return value.amount === null ? null : toBytes(value.amount, value.unit);
}

export function isWithinByteRange(value: BytesValue): boolean {
  const bytes = bytesOf(value);
  return bytes !== null && Number.isFinite(bytes) && bytes >= 0 && bytes <= MAX_SAFE_BYTES;
}

export interface OptionalBytesValue {
  unlimited: boolean;
  amount: BytesValue;
}

interface OptionalBytesInputProps extends FieldControlProps {
  value: OptionalBytesValue;
  onChange: (value: OptionalBytesValue) => void;
  unlimitedLabel: string;
  disabled?: boolean;
}

export function OptionalBytesInput({
  value,
  onChange,
  unlimitedLabel,
  disabled = false,
  ...control
}: OptionalBytesInputProps) {
  return (
    <Flex vertical gap={8}>
      <Flex align="center" gap={8}>
        <Switch
          checked={value.unlimited}
          disabled={disabled}
          aria-label={unlimitedLabel}
          onChange={(unlimited) => {
            onChange({ ...value, unlimited });
          }}
        />
        <span aria-hidden="true">{unlimitedLabel}</span>
      </Flex>
      <BytesInput
        {...control}
        value={value.amount}
        disabled={disabled || value.unlimited}
        onChange={(amount) => {
          onChange({ ...value, amount });
        }}
      />
    </Flex>
  );
}

export function optionalBytesOf(value: OptionalBytesValue): number | null {
  return value.unlimited ? null : bytesOf(value.amount);
}
