import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest, { params }: { params: Promise<{ objectPath: string[] }> }) {
  const { objectPath } = await params;
  const cookieHeader = req.headers.get("cookie");
  const objectName = objectPath.join("/");
  const url = `${API_BASE_URL}/files/${encodeURIComponent(objectName)}/download`;

  const apiRes = await fetch(url, {
    method: "GET",
    headers: {
      cookie: cookieHeader || "",
    },
    redirect: "manual",
  });

  if (!apiRes.ok) {
    const resBody = await apiRes.text();
    return new NextResponse(resBody, {
      status: apiRes.status,
      headers: {
        "Content-Type": "application/json",
      },
    });
  }

  const res = new NextResponse(apiRes.body, {
    status: apiRes.status,
    headers: {
      "Content-Type": apiRes.headers.get("Content-Type") || "application/octet-stream",
      "Content-Length": apiRes.headers.get("Content-Length") || "",
      "Accept-Ranges": apiRes.headers.get("Accept-Ranges") || "",
      "Content-Range": apiRes.headers.get("Content-Range") || "",
    },
  });

  const setCookie = apiRes.headers.getSetCookie?.() || [];
  if (setCookie.length > 0) {
    res.headers.set("Set-Cookie", setCookie.join(","));
  }

  return res;
}
