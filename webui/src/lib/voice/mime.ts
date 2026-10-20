/** The media type of the instance's TTS output; mirrors `audio_mime` on the server. */
export function ttsMime(format: string): string {
	switch (format) {
		case 'opus':
			return 'audio/ogg';
		case 'mp3':
			return 'audio/mpeg';
		case 'pcm':
			return 'audio/L16';
		default:
			return `audio/${format}`;
	}
}
