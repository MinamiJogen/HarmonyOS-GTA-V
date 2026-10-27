/** Diagnostic storage is bounded even when tracing is explicitly enabled. */
export const TOUCH_TRACE_CAPACITY: number = 16;
export class TouchTraceRing<T> {
  private entries: T[] = [];
  private cursor: number = 0;
  push(value: T): void {
    this.entries[this.cursor] = value;
    this.cursor = (this.cursor + 1) % TOUCH_TRACE_CAPACITY;
  }
  snapshot(): T[] {
    if (this.entries.length < TOUCH_TRACE_CAPACITY) { return this.entries.slice(); }
    const ordered: T[] = [];
    for (let offset: number = 0; offset < TOUCH_TRACE_CAPACITY; offset++) {
      ordered.push(this.entries[(this.cursor + offset) % TOUCH_TRACE_CAPACITY]);
    }
    return ordered;
  }
  clear(): void { this.entries = []; this.cursor = 0; }
}
