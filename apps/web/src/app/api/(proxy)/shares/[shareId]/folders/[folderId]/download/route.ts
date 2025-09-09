import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest, { params }: { params: Promise<{ shareId: string; folderId: string }> }) {
  const { shareId, folderId } = await params;
  const cookieHeader = req.headers.get("cookie");
  const url = new URL(req.url);
  const searchParams = url.searchParams.toString();
  const fetchUrl = `${API_BASE_URL}/shares/${shareId}/folders/${folderId}/download${searchParams ? `?${searchParams}` : ""}`;

  const apiRes = await fetch(fetchUrl, {
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
      "Content-Type": apiRes.headers.get("Content-Type") || "application/zip",
      "Content-Length": apiRes.headers.get("Content-Length") || "",
      "Content-Disposition": apiRes.headers.get("Content-Disposition") || "",
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
