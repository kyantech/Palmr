import { FastifyReply, FastifyRequest } from "fastify";

import { ConfigService } from "../config/service";

const configService = new ConfigService();

export async function validatePasswordMiddleware(request: FastifyRequest, reply: FastifyReply) {
  const body = request.body as any;
  if (!body.password) return;

  const minLength = Number(await configService.getValue("passwordMinLength"));

  if (body.password.length < minLength) {
    return reply.status(400).send({
      error: `Password must be at least ${minLength} characters long`,
    });
  }
}
