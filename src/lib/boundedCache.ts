/** LRU with both an entry cap and a caller-defined weight budget. Oversized
 * values remain usable by their caller but are never retained by the cache. */
export class BoundedCache<V> {
  private entries = new Map<string, { value: V; weight: number }>();
  private weight = 0;
  constructor(
    private maxEntries: number,
    private maxWeight: number,
    private weigh: (key: string, value: V) => number,
  ) {}
  get size() { return this.entries.size; }
  get totalWeight() { return this.weight; }
  get(key: string): V | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.entries.delete(key);
    this.entries.set(key, entry);
    return entry.value;
  }
  set(key: string, value: V): void {
    this.delete(key);
    const weight = this.weigh(key, value);
    if (!Number.isFinite(weight) || weight < 0 || weight > this.maxWeight) return;
    this.entries.set(key, { value, weight });
    this.weight += weight;
    while (this.entries.size > this.maxEntries || this.weight > this.maxWeight) {
      this.delete(this.entries.keys().next().value!);
    }
  }
  delete(key: string): void {
    const entry = this.entries.get(key);
    if (entry) { this.weight -= entry.weight; this.entries.delete(key); }
  }
}
