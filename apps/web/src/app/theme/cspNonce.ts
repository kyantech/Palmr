export const CSP_NONCE_META = 'meta[name="csp-nonce"]';

export function readCspNonce(root: ParentNode = document): string | undefined {
  const nonce = root.querySelector<HTMLMetaElement>(CSP_NONCE_META)?.content.trim();
  return nonce === "" ? undefined : nonce;
}
