import { NextRequest, NextResponse } from "next/server";

export async function PATCH(req: NextRequest, { params }: { params: { key: string } }) {
  const key = params.key;
  const body = await req.text();
  const cookieHeader = req.headers.get("cookie");

  const apiRes = await fetch(`${process.env.API_BASE_URL}/app/configs/${key}`, {
    method: "PATCH",
    headers: {
      "Content-Type": "application/json",
      cookie: cookieHeader || "",
    },
    body,
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
