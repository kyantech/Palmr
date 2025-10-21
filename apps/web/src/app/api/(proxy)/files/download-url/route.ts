import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest) {
  const cookieHeader = req.headers.get("cookie");
  const searchParams = req.nextUrl.searchParams;
  const objectName = searchParams.get("objectName");

  if (!objectName) {
    return new NextResponse(JSON.stringify({ error: "objectName parameter is required" }), {
      status: 400,
      headers: {
        "Content-Type": "application/json",
      },
    });
  }

  // Forward all query params to backend
  const queryString = searchParams.toString();
  const url = `${API_BASE_URL}/files/download-url?${queryString}`;

  const apiRes = await fetch(url, {
    method: "GET",
    headers: {
      cookie: cookieHeader || "",
    },
  });

  const data = await apiRes.json();

  return new NextResponse(JSON.stringify(data), {
    status: apiRes.status,
    headers: {
      "Content-Type": "application/json",
    },
  });
}
