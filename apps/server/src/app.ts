import crypto from "node:crypto";
import fastifyCookie from "@fastify/cookie";
import { fastifyCors } from "@fastify/cors";
import fastifyJwt from "@fastify/jwt";
import { fastifySwaggerUi } from "@fastify/swagger-ui";
import { fastify } from "fastify";
import { serializerCompiler, validatorCompiler, ZodTypeProvider } from "fastify-type-provider-zod";

import { registerSwagger } from "./config/swagger.config";
import { envTimeoutOverrides } from "./config/timeout.config";
import { prisma } from "./shared/prisma";

export async function buildApp() {
  const jwtConfig = await prisma.appConfig.findUnique({
    where: { key: "jwtSecret" },
  });

  const JWT_SECRET = jwtConfig?.value || crypto.randomBytes(64).toString("hex");

  const app = fastify({
    ajv: {
      customOptions: {
        removeAdditional: false,
      },
    },
    logger: {
      level: "warn",
    },
    bodyLimit: 1024 * 1024 * 1024 * 1024 * 1024,
    connectionTimeout: 0,
    keepAliveTimeout: envTimeoutOverrides.keepAliveTimeout,
    requestTimeout: envTimeoutOverrides.requestTimeout,
    trustProxy: true,
  }).withTypeProvider<ZodTypeProvider>();

  app.setValidatorCompiler(validatorCompiler);
  app.setSerializerCompiler(serializerCompiler);

  app.addSchema({
    $id: "dateFormat",
    type: "string",
    format: "date-time",
  });

  app.register(fastifyCors, {
    origin: true,
    credentials: true,
  });

  app.register(fastifyCookie);
  app.register(fastifyJwt, {
    secret: JWT_SECRET,
    cookie: {
      cookieName: "token",
      signed: false,
    },
    sign: {
      expiresIn: "1d",
    },
  });

  app.decorateRequest("jwtSign", function (this: any, payload: object, options?: object) {
    return this.server.jwt.sign(payload, options);
  });

  registerSwagger(app);
  app.register(fastifySwaggerUi, {
    routePrefix: "/swagger",
  });

  app.register(require("@scalar/fastify-api-reference"), {
    routePrefix: "/docs",
    configuration: {
      theme: "deepSpace",
    },
  });

  return app;
}
