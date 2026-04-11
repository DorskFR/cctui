export type LineCallback = (line: string) => void | Promise<void>;

/**
 * Tail the Claude Code session.jsonl transcript file and forward each raw JSONL
 * line to the server via the callback.  Parsing happens server-side so we store
 * a lossless copy of every transcript line.
 */
export async function tailTranscript(
  transcriptPath: string,
  onLine: LineCallback,
  signal?: AbortSignal,
): Promise<void> {
  // Wait up to 30 s for the transcript file to appear.
  for (let i = 0; i < 60; i++) {
    if (signal?.aborted) return;
    const exists = await Bun.file(transcriptPath).exists();
    if (exists) break;
    if (i === 59) return;
    await Bun.sleep(500);
  }

  let offset = 0;

  const processNewContent = async () => {
    const file = Bun.file(transcriptPath);
    if (file.size <= offset) return;

    const content = await file.text();
    const newContent = content.slice(offset);
    offset = content.length;

    for (const line of newContent.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      await onLine(trimmed);
    }
  };

  await processNewContent();

  while (!signal?.aborted) {
    await Bun.sleep(300);
    await processNewContent();
  }
}
