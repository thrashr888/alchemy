/** Recently used gallery thumbnails. Encoded images count conservatively as
 * UTF-16; entry count also bounds empty results. Disk caching lives in Rust. */
export class ThumbnailCache {
  private entries = new Map<string, string>();
  private bytes = 0;
  constructor(private maxBytes = 16 * 1024 * 1024, private maxEntries = 128) {}
  get size() { return this.entries.size; }
  get byteLength() { return this.bytes; }
  has(key: string) { return this.entries.has(key); }
  get(key: string) {
    const value = this.entries.get(key);
    if (value !== undefined) {
      this.entries.delete(key);
      this.entries.set(key, value);
    }
    return value;
  }
  set(key: string, value: string) {
    this.delete(key);
    const bytes = (key.length + value.length) * 2;
    if (bytes > this.maxBytes) return;
    this.entries.set(key, value);
    this.bytes += bytes;
    while (this.bytes > this.maxBytes || this.entries.size > this.maxEntries) {
      this.delete(this.entries.keys().next().value!);
    }
  }
  delete(key: string) {
    const value = this.entries.get(key);
    if (value === undefined) return false;
    this.bytes -= (key.length + value.length) * 2;
    return this.entries.delete(key);
  }
  clear() { this.entries.clear(); this.bytes = 0; }
}
export const thumbMemory = new ThumbnailCache();
