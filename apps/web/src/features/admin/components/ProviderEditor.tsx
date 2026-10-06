import { Drawer } from "antd";
import { useTranslation } from "react-i18next";
import type { Provider } from "../types";
import { ProviderForm } from "./ProviderForm";

export type ProviderEditorTarget = { mode: "create" } | { mode: "edit"; provider: Provider };

interface ProviderEditorProps {
  target: ProviderEditorTarget | null;
  onClose: () => void;
}

export function ProviderEditor({ target, onClose }: ProviderEditorProps) {
  const { t } = useTranslation("admin");
  return (
    <Drawer
      open={target !== null}
      onClose={onClose}
      destroyOnHidden
      size={560}
      mask={{ closable: false }}
      title={
        target?.mode === "edit"
          ? t("providers.editor.editTitle", { name: target.provider.displayName })
          : t("providers.editor.createTitle")
      }
    >
      {target === null ? null : (
        <ProviderForm
          key={target.mode === "edit" ? target.provider.id : "create"}
          mode={target.mode}
          provider={target.mode === "edit" ? target.provider : null}
          onClose={onClose}
        />
      )}
    </Drawer>
  );
}
