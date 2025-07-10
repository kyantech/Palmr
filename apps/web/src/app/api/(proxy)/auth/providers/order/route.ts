import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function PUT(request: NextRequest) {
  try {
    const body = await request.json();
    const url = `${API_BASE_URL}/auth/providers/order`;

    const apiRes = await fetch(url, {
      method: "PUT",
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
    console.error("Error proxying auth providers order request:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
