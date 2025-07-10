import type { FastifyRequest } from "fastify";

declare module "fastify" {
  interface FastifyRequest {
    /**
     * Método decorado para assinar um payload JWT.
     * @param payload - Objeto que será assinado.
     * @param options - Opções adicionais para a assinatura.
     * @returns O token JWT assinado.
     */
    jwtSign(payload: object, options?: object): string;
  }
}
