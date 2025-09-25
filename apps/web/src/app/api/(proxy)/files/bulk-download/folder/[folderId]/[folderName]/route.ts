import { NextRequest, NextResponse } from "next/server";

const API_BASE_URL = process.env.API_BASE_URL || "http://localhost:3333";

export async function GET(
  request: NextRequest,
  { params }: { params: Promise<{ folderId: string; folderName: string }> }
) {
  try {
    const cookieHeader = request.headers.get("cookie");
    const { folderId, folderName } = await params;

    const apiRes = await fetch(`${API_BASE_URL}/bulk-download/folder/${folderId}/${folderName}`, {
      method: "GET",
      headers: {
        cookie: cookieHeader || "",
      },
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
        "Content-Disposition": `attachment; filename=${folderName}.zip`,
      },
    });
  } catch (error) {
    console.error("Folder download proxy error:", error);
    return NextResponse.json({ error: "Internal server error" }, { status: 500 });
  }
}
