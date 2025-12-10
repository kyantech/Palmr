import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(request: NextRequest) {
  try {
    const url = new URL(request.url);
    const queryString = url.search;

    const originalHost = request.headers.get("host") || url.host;
    // Handle multiple protocols in x-forwarded-proto (e.g., "https, https" from multiple proxies)
    const forwardedProto = request.headers.get("x-forwarded-proto");
    const originalProtocol = forwardedProto ? forwardedProto.split(",")[0].trim() : url.protocol.replace(":", "");
    const listUrl = `${API_BASE_URL}/auth/providers${queryString}`;

    const apiRes = await fetch(listUrl, {
      method: "GET",
      headers: {
        "Content-Type": "application/json",
        "x-forwarded-host": originalHost,
        "x-forwarded-proto": originalProtocol,
        cookie: request.headers.get("cookie") || "",
        ...Object.fromEntries(Array.from(request.headers.entries()).filter(([key]) => key.startsWith("authorization"))),
      },
    });

    const data = await apiRes.json();

    return NextResponse.json(data, {
      status: apiRes.status,
      headers: {
        "Content-Type": "application/json",
      },
    });
  } catch (error) {
    console.error("Error proxying auth providers request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}

export async function POST(request: NextRequest) {
  try {
    const body = await request.json();
    const createUrl = `${API_BASE_URL}/auth/providers`;

    const apiRes = await fetch(createUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        cookie: request.headers.get("cookie") || "",
        ...Object.fromEntries(Array.from(request.headers.entries()).filter(([key]) => key.startsWith("authorization"))),
      },
      body: JSON.stringify(body),
    });

    const data = await apiRes.json();

    return NextResponse.json(data, {
      status: apiRes.status,
      headers: {
        "Content-Type": "application/json",
      },
    });
  } catch (error) {
    console.error("Error proxying auth providers POST request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
