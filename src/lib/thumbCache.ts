// Resolved gallery-card visuals (data URIs) per source id, so reopening the
// gallery paints instantly instead of re-running IPC + image fetches.
// "" = checked, none. The backend also disk-caches og downloads. Lives
// outside GalleryPane so the reader's image picker can invalidate a card
// after the user hand-picks a new image.
export const thumbMemory = new Map<string, string>();
