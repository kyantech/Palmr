import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest) {
  const cookieHeader = req.headers.get("cookie");

  // Get the multipart form data directly
  const formData = await req.formData();

  const url = `${API_BASE_URL}/shares/create-with-files`;

  const apiRes = await fetch(url, {
    method: "POST",
    headers: {
      cookie: cookieHeader || "",
      // Don't set Content-Type, let fetch set it with boundary
    },
    body: formData,
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
