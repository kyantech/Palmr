import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function PUT(request: NextRequest, { params }: { params: Promise<{ id: string }> }) {
  try {
    const body = await request.json();
    const { id } = await params;
    const updateUrl = `${API_BASE_URL}/auth/providers/${id}`;

    const apiRes = await fetch(updateUrl, {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        ...Object.fromEntries(
          Array.from(request.headers.entries()).filter(
            ([key]) => key.startsWith("authorization") || key.startsWith("cookie")
          )
        ),
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
    console.error("Error proxying auth provider update:", error);
    return NextResponse.json({ success: false, error: "Internal server error" }, { status: 500 });
  }
}

export async function DELETE(request: NextRequest, { params }: { params: Promise<{ id: string }> }) {
  try {
    const { id } = await params;
    const deleteUrl = `${API_BASE_URL}/auth/providers/${id}`;

    const apiRes = await fetch(deleteUrl, {
      method: "DELETE",
      headers: {
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
    console.error("Error proxying auth provider delete:", error);
    return NextResponse.json({ success: false, error: "Internal server error" }, { status: 500 });
  }
}
