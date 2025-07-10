import { NextResponse } from "next/server";

export async function GET() {
  return new NextResponse(
    JSON.stringify({
      error: "This endpoint has been deprecated for security reasons. Use secure server actions instead.",
      message: "Please use getSecureConfigs() or getAdminConfigs() server actions",
    }),
    {
      status: 410,
      headers: {
        "Content-Type": "application/json",
      },
    }
  );
}
