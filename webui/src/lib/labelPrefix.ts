/** Count visible characters without splitting accents or emoji sequences. */
export function labelPrefix(label: string, length: number): string {
  return Array.from(
    new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(label),
    (part) => part.segment,
  ).slice(0, length).join("");
}
