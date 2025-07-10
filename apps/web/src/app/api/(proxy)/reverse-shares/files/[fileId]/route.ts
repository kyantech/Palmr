import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(req: NextRequest, { params }: { params: Promise<{ fileId: string }> }) {
  const cookieHeader = req.headers.get("cookie");
  const { fileId } = await params;
  const url = `${API_BASE_URL}/reverse-shares/files/${fileId}/download`;

  const apiRes = await fetch(url, {
    method: "GET",
    headers: {
      "Content-Type": "application/json",
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

export async function PUT(req: NextRequest, { params }: { params: Promise<{ fileId: string }> }) {
  const cookieHeader = req.headers.get("cookie");
  const { fileId } = await params;
  const body = await req.json();
  const url = `${API_BASE_URL}/reverse-shares/files/${fileId}`;

  const apiRes = await fetch(url, {
    method: "PUT",
    headers: {
      "Content-Type": "application/json",
      cookie: cookieHeader || "",
    },
    body: JSON.stringify(body),
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

export async function DELETE(req: NextRequest, { params }: { params: Promise<{ fileId: string }> }) {
  const cookieHeader = req.headers.get("cookie");
  const { fileId } = await params;
  const url = `${API_BASE_URL}/reverse-shares/files/${fileId}`;

  const apiRes = await fetch(url, {
    method: "DELETE",
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
