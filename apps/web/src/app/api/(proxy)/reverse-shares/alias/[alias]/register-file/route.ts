import { NextRequest, NextResponse } from "next/server";

export const dynamic = "force-dynamic";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(req: NextRequest, { params }: { params: Promise<{ alias: string }> }) {
  const { searchParams } = new URL(req.url);
  const password = searchParams.get("password");
  const body = await req.text();
  const { alias } = await params;

  let url = `${API_BASE_URL}/reverse-shares/alias/${alias}/register-file`;
  if (password) {
    url += `?password=${encodeURIComponent(password)}`;
  }

  const apiRes = await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
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

  return res;
}
