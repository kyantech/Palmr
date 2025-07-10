import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function PATCH(req: NextRequest, { params }: { params: Promise<{ id: string }> }) {
  const cookieHeader = req.headers.get("cookie");
  const { id } = await params;
  const url = `${API_BASE_URL}/users/${id}/deactivate`;

  const apiRes = await fetch(url, {
    method: "PATCH",
    headers: {
      cookie: cookieHeader || "",
    },
    redirect: "manual",
  });

  const resBody = await apiRes.text();

  const res = new NextResponse(resBody, {
    status: apiRes.status,
    headers: {
      "Content-Type": "application/json",
    },
  });

  const setCookie = apiRes.headers.getSetCookie?.() || [];
  if (setCookie.length > 0) {
    res.headers.set("Set-Cookie", setCookie.join(","));
  }

  return res;
}
