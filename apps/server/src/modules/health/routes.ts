import { FastifyInstance } from "fastify";
import { z } from "zod";

import { HealthController } from "./controller";

export async function healthRoutes(app: FastifyInstance) {
  const healthController = new HealthController();

  app.get(
    "/health",
    {
      schema: {
        tags: ["Health"],
        operationId: "checkHealth",
        summary: "Check API Health",
        description: "Returns the health status of the API",
        response: {
          200: z.object({
            status: z.string().describe("The health status"),
            timestamp: z.string().describe("The timestamp of the health check"),
          }),
        },
      },
    },
    async () => healthController.check()
  );
}
