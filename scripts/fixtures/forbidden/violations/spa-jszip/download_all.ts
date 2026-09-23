import JSZip from "jszip";

export async function downloadAll(files: File[]): Promise<Blob> {
  const zip = new JSZip();
  for (const file of files) {
    zip.file(file.name, file);
  }
  return zip.generateAsync({ type: "blob" });
}
