import { useEffect, useState } from "react";

import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

export interface FileSizeInputProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  error?: any;
  placeholder?: string;
}

type Unit = "MB" | "GB" | "TB" | "PB";

const UNIT_MULTIPLIERS: Record<Unit, number> = {
  MB: 1024 * 1024,
  GB: 1024 * 1024 * 1024,
  TB: 1024 * 1024 * 1024 * 1024,
  PB: 1024 * 1024 * 1024 * 1024 * 1024,
};

function bytesToHumanReadable(bytes: string): { value: string; unit: Unit } {
  const numBytes = parseInt(bytes, 10);

  if (!numBytes || numBytes <= 0) {
    return { value: "0", unit: "MB" };
  }

  const units: Unit[] = ["PB", "TB", "GB", "MB"];

  for (const unit of units) {
    const multiplier = UNIT_MULTIPLIERS[unit];
    const value = numBytes / multiplier;

    if (value >= 1) {
      const rounded = Math.round(value * 100) / 100;

      if (Math.abs(rounded - Math.round(rounded)) < 0.01) {
        return { value: Math.round(rounded).toString(), unit };
      } else {
        return { value: rounded.toFixed(2), unit };
      }
    }
  }

  const mbValue = numBytes / UNIT_MULTIPLIERS.MB;
  return { value: mbValue.toFixed(2), unit: "MB" as Unit };
}

function humanReadableToBytes(value: string, unit: Unit): string {
  const numValue = parseFloat(value);
  if (isNaN(numValue) || numValue <= 0) {
    return "0";
  }

  return Math.floor(numValue * UNIT_MULTIPLIERS[unit]).toString();
}

export function FileSizeInput({ value, onChange, disabled = false, error, placeholder = "0" }: FileSizeInputProps) {
  const [displayValue, setDisplayValue] = useState("");
  const [selectedUnit, setSelectedUnit] = useState<Unit>("MB");

  useEffect(() => {
    if (value && value !== "0") {
      const { value: humanValue, unit } = bytesToHumanReadable(value);
      setDisplayValue(humanValue);
      setSelectedUnit(unit);
    } else {
      setDisplayValue("");
      setSelectedUnit("MB");
    }
  }, [value]);

  const handleValueChange = (newValue: string) => {
    const sanitizedValue = newValue.replace(/[^0-9.]/g, "");

    const parts = sanitizedValue.split(".");
    const finalValue = parts.length > 2 ? parts[0] + "." + parts.slice(1).join("") : sanitizedValue;

    setDisplayValue(finalValue);

    if (finalValue === "" || finalValue === "0") {
      onChange("0");
    } else {
      const bytesValue = humanReadableToBytes(finalValue, selectedUnit);
      onChange(bytesValue);
    }
  };

  const handleUnitChange = (newUnit: Unit) => {
    if (!newUnit || !["MB", "GB", "TB", "PB"].includes(newUnit)) {
      return;
    }

    setSelectedUnit(newUnit);

    if (displayValue && displayValue !== "0") {
      const bytesValue = humanReadableToBytes(displayValue, newUnit);
      onChange(bytesValue);
    }
  };

  return (
    <div className="flex gap-2">
      <Input
        type="text"
        value={displayValue}
        onChange={(e) => handleValueChange(e.target.value)}
        placeholder={placeholder}
        className="flex-1"
        disabled={disabled}
        aria-invalid={!!error}
      />
      <Select
        key={`${selectedUnit}-${displayValue}`}
        value={selectedUnit}
        onValueChange={handleUnitChange}
        disabled={disabled}
      >
        <SelectTrigger className="w-20">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="MB">MB</SelectItem>
          <SelectItem value="GB">GB</SelectItem>
          <SelectItem value="TB">TB</SelectItem>
          <SelectItem value="PB">PB</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}
