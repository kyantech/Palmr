import { NextRequest, NextResponse } from "next/server";

import { detectMimeTypeWithFallback } from "@/utils/mime-types";

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

  const serverContentType = apiRes.headers.get("Content-Type");
  const contentDisposition = apiRes.headers.get("Content-Disposition");
  const contentLength = apiRes.headers.get("Content-Length");
  const acceptRanges = apiRes.headers.get("Accept-Ranges");
  const contentRange = apiRes.headers.get("Content-Range");
  const contentType = detectMimeTypeWithFallback(serverContentType, contentDisposition, objectName);

  const res = new NextResponse(apiRes.body, {
    status: apiRes.status,
    headers: {
      "Content-Type": contentType,
      ...(contentLength && { "Content-Length": contentLength }),
      ...(acceptRanges && { "Accept-Ranges": acceptRanges }),
      ...(contentRange && { "Content-Range": contentRange }),
      ...(contentDisposition && { "Content-Disposition": contentDisposition }),
    },
  });

  const setCookie = apiRes.headers.getSetCookie?.() || [];
  if (setCookie.length > 0) {
    res.headers.set("Set-Cookie", setCookie.join(","));
  }

  return res;
}
