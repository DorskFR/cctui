import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { CreateProfileRequest } from "@bindings/CreateProfileRequest";
import type { SessionProfile } from "@bindings/SessionProfile";
import type { UpdateProfileRequest } from "@bindings/UpdateProfileRequest";
import { endpoints } from "./endpoints";

export const PROFILES_KEY = ["profiles"] as const;

/** The caller's spawn profiles (the radio list in the spawn panel). */
export const useProfiles = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: PROFILES_KEY,
    queryFn: endpoints.profiles,
    enabled: enabled(),
  }));

/** Create / adjust / delete profiles; each invalidates the list. */
export function useProfileActions() {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: PROFILES_KEY });
  return {
    create: async (body: CreateProfileRequest): Promise<SessionProfile> => {
      const p = await endpoints.createProfile(body);
      await invalidate();
      return p;
    },
    update: async (
      id: string,
      body: UpdateProfileRequest,
    ): Promise<SessionProfile> => {
      const p = await endpoints.updateProfile(id, body);
      await invalidate();
      return p;
    },
    remove: async (id: string): Promise<void> => {
      await endpoints.deleteProfile(id);
      await invalidate();
    },
  };
}
