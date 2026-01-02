import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(
  req: NextRequest,
  { params }: { params: Promise<{ alias: string }> }
) {
  const { searchParams } = new URL(req.url);
  const { alias } = await params;

  // Forward all query params (password, uploadId, objectName, partNumber)
  const url = new URL(`${API_BASE_URL}/reverse-shares/alias/${alias}/multipart/part-url`);
  searchParams.forEach((value, key) => url.searchParams.set(key, value));

  const apiRes = await fetch(url.toString(), {
    method: "GET",
    redirect: "manual",
  });

  const resBody = await apiRes.text();

  return new NextResponse(resBody, {
    status: apiRes.status,
    headers: { "Content-Type": "application/json" },
  });
}
