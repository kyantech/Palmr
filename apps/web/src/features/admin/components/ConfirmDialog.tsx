import { Modal } from "antd";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  danger?: boolean;
  loading?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  danger = false,
  loading = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const { t } = useTranslation("admin");
  return (
    <Modal
      open={open}
      title={title}
      okText={confirmLabel}
      cancelText={t("common.cancel")}
      okButtonProps={{ danger, loading }}
      cancelButtonProps={{ disabled: loading }}
      onOk={onConfirm}
      onCancel={onCancel}
      centered
      destroyOnHidden
      width={440}
      mask={{ closable: !loading }}
      keyboard={!loading}
    >
      {description}
    </Modal>
  );
}
