/** Push-to-talk dictation: record the microphone while held, transcribe
 *  the clip through `/voice/transcribe`. */
import { api } from '$lib/api';
import type { VoiceTranscript } from '@bindings/VoiceTranscript';

const PREFERRED = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', 'audio/ogg;codecs=opus'];

export function pickMime(supported: (t: string) => boolean): string | undefined {
	return PREFERRED.find(supported);
}

export async function transcribeBlob(blob: Blob, language?: string): Promise<string> {
	const form = new FormData();
	form.append('file', blob, 'clip.webm');
	if (language) form.append('language', language);
	const res = await api.postForm<VoiceTranscript>('/voice/transcribe', form);
	return res.text.trim();
}

export class Dictation {
	private recorder: MediaRecorder | null = null;
	private chunks: Blob[] = [];
	private stream: MediaStream | null = null;

	get recording(): boolean {
		return this.recorder?.state === 'recording';
	}

	async start() {
		if (this.recorder) return;
		this.stream = await navigator.mediaDevices.getUserMedia({ audio: true });
		const mimeType = pickMime((t) => MediaRecorder.isTypeSupported(t));
		this.recorder = new MediaRecorder(this.stream, mimeType ? { mimeType } : undefined);
		this.chunks = [];
		this.recorder.ondataavailable = (e) => {
			if (e.data.size) this.chunks.push(e.data);
		};
		this.recorder.start();
	}

	/** The transcript of what was recorded, empty when nothing was heard. */
	async stop(): Promise<string> {
		const rec = this.recorder;
		if (!rec) return '';
		this.recorder = null;
		const clip = await new Promise<Blob>((resolve) => {
			rec.onstop = () => resolve(new Blob(this.chunks, { type: rec.mimeType }));
			rec.stop();
		});
		this.release();
		return clip.size ? transcribeBlob(clip) : '';
	}

	cancel() {
		const rec = this.recorder;
		this.recorder = null;
		if (rec && rec.state !== 'inactive') rec.stop();
		this.release();
	}

	private release() {
		for (const t of this.stream?.getTracks() ?? []) t.stop();
		this.stream = null;
		this.chunks = [];
	}
}
