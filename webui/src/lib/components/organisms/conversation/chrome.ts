/** Which shell a `ConversationPane` is mounted in. `drawer` is the side panel
 *  (back chevron, document scroll locked); `tile` is one cell of the Sessions
 *  tiles grid (no back control, maximize toggle, scrolls inside the cell). */
export type ConversationChrome = 'drawer' | 'tile';
