export async function readWholeBody(response: Response): Promise<Blob> {
  return response.blob();
}
