interface FieldError {
  type: string;
  message?: string;
}

export function hasMessage(error: FieldError): boolean {
  return error.message !== undefined;
}
