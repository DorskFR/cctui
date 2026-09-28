import { ApiError } from "./api";

/** The message to show inline when a settings save fails, or null when the
 *  failure is a network / 5xx one the user can only be told to retry. */
export function saveErrorMessage(e: unknown): string | null {
  if (!(e instanceof ApiError)) return null;
  return e.status >= 400 && e.status < 500 ? e.message : null;
}
