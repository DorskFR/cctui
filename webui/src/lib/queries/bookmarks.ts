import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { Bookmark } from "@bindings/Bookmark";
import type { CreateBookmark } from "@bindings/CreateBookmark";
import type { UpdateBookmark } from "@bindings/UpdateBookmark";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

/** Saved messages, newest first; `q` is applied server-side. */
export const useBookmarks = (q: () => string = () => "") =>
  createQuery(() => ({
    queryKey: qk.bookmarks(q()),
    queryFn: () => endpoints.bookmarks(q()),
  }));

export function useBookmarkActions() {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: ["bookmarks"] });
  return {
    create: async (body: CreateBookmark): Promise<Bookmark> => {
      const b = await endpoints.createBookmark(body);
      await invalidate();
      return b;
    },
    update: async (id: string, body: UpdateBookmark): Promise<Bookmark> => {
      const b = await endpoints.updateBookmark(id, body);
      await invalidate();
      return b;
    },
    remove: async (id: string): Promise<void> => {
      await endpoints.deleteBookmark(id);
      await invalidate();
    },
  };
}
