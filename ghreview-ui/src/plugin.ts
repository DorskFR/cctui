import type { CctuiPluginModule } from "./lib/plugin/host";
import ReviewPage from "./ReviewPage.svelte";

export default {
  cctuiApi: 1,
  page: ReviewPage,
} satisfies CctuiPluginModule;
