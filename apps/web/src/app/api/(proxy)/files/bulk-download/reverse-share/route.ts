import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function POST(request: NextRequest) {
  try {
    const cookieHeader = request.headers.get("cookie");
    const body = await request.text();

    const apiRes = await fetch(`${API_BASE_URL}/bulk-download/reverse-share`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        cookie: cookieHeader || "",
      },
      body,
    });

    if (!apiRes.ok) {
      const errorText = await apiRes.text();
      return NextResponse.json({ error: errorText }, { status: apiRes.status });
    }

    // For binary responses (ZIP files), we need to handle them differently
    const buffer = await apiRes.arrayBuffer();

    return new NextResponse(buffer, {
      status: 200,
      headers: {
        "Content-Type": "application/zip",
        "Content-Disposition": apiRes.headers.get("Content-Disposition") || "attachment; filename=download.zip",
      },
    });
  } catch (error) {
    console.error("Reverse share bulk download proxy error:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
