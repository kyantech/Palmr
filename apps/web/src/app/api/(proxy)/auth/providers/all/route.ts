import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(request: NextRequest) {
  const url = `${API_BASE_URL}/auth/providers/all`;
  try {
    const apiRes = await fetch(url, {
      method: "GET",
      headers: {
        "Content-Type": "application/json",
        ...Object.fromEntries(
          Array.from(request.headers.entries()).filter(
            ([key]) => key.startsWith("authorization") || key.startsWith("cookie")
          )
        ),
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
    console.error("Error proxying auth providers all request:", error);
    return NextResponse.json({ success: false, error: "Internal server error" }, { status: 500 });
  }
}
