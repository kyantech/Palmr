import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function DELETE(req: NextRequest, { params }: { params: Promise<{ fileId: string }> }) {
  const { fileId } = await params;
  const cookieHeader = req.headers.get("cookie");
  const url = `${API_BASE_URL}/filesystem/cancel-upload/${fileId}`;

  const apiRes = await fetch(url, {
    method: "DELETE",
    headers: {
      cookie: cookieHeader || "",
    },
  });

  const contentType = apiRes.headers.get("Content-Type") || "application/json";
  const resBody = await apiRes.text();

  const res = new NextResponse(resBody, {
    status: apiRes.status,
    headers: {
      "Content-Type": contentType,
    },
  });

  const setCookie = apiRes.headers.getSetCookie?.() || [];
  if (setCookie.length > 0) {
    res.headers.set("Set-Cookie", setCookie.join(","));
  }

  return res;
}
