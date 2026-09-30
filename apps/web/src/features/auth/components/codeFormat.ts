const TOTP_CODE = /^\d{6}$/;
const BACKUP_CODE = /^[A-Z2-7]{16}$/;

export const TOTP_CODE_LENGTH = 6;
export const BACKUP_CODE_DISPLAY_LENGTH = 19;

export function compactCode(value: string): string {
  return value.replace(/\s+/g, "");
}

export function isTotpCode(value: string): boolean {
  return TOTP_CODE.test(compactCode(value));
}

export function isBackupCode(value: string): boolean {
  return BACKUP_CODE.test(compactCode(value).replaceAll("-", "").toUpperCase());
}

export function groupSecret(secret: string): string {
  return (secret.match(/.{1,4}/g) ?? []).join(" ");
}
