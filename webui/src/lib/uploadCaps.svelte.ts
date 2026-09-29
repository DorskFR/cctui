import type { UploadCaps } from '@bindings/UploadCaps';
import { DEFAULT_UPLOAD_CAPS } from './attachments';

// The caps the server is enforcing, as served on `GET /version`. Held here
// rather than threaded through props so the composer and the spawn modal
// pre-flight against the same values without either owning the fetch.
export const uploadCaps = $state<UploadCaps>({ ...DEFAULT_UPLOAD_CAPS });

export function setUploadCaps(next: UploadCaps | null | undefined) {
	Object.assign(uploadCaps, next ?? DEFAULT_UPLOAD_CAPS);
}
