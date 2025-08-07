import { NextRequest, NextResponse } from "next/server";

import { getClientHeaders } from "@/lib/proxy-utils";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function DELETE(request: NextRequest, { params }: { params: Promise<{ id: string }> }) {
  try {
    const { id } = await params;
    const deleteUrl = `${API_BASE_URL}/auth/trusted-devices/${id}`;

    const clientHeaders = getClientHeaders(request);

    const apiRes = await fetch(deleteUrl, {
      method: "DELETE",
      headers: {
        ...clientHeaders,
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
    console.error("Error proxying trusted device delete:", error);
    return NextResponse.json({ success: false, error: "Internal server error" }, { status: 500 });
  }
}
