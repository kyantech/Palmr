import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest, { params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/filesystem/download/${token}`;

  const apiRes = await fetch(url, {
    method: "GET",
    headers: {
      cookie: cookieHeader || "",
    },
  });

  const contentType = apiRes.headers.get("Content-Type") || "application/octet-stream";
  const contentDisposition = apiRes.headers.get("Content-Disposition");
  const contentLength = apiRes.headers.get("Content-Length");

  const res = new NextResponse(apiRes.body, {
    status: apiRes.status,
    headers: {
      "Content-Type": contentType,
      ...(contentDisposition && { "Content-Disposition": contentDisposition }),
      ...(contentLength && { "Content-Length": contentLength }),
    },
  });

  const setCookie = apiRes.headers.getSetCookie?.() || [];
  if (setCookie.length > 0) {
    res.headers.set("Set-Cookie", setCookie.join(","));
  }

  return res;
}
