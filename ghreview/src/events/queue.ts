export const MAX_QUEUED_EVENTS = 512;

export class EventQueue<T> {
  private items: T[] = [];
  private waiter: (() => void) | null = null;
  private stopped = false;
  private droppedCount = 0;

  constructor(private readonly max: number = MAX_QUEUED_EVENTS) {}

  get size(): number {
    return this.items.length;
  }

  get dropped(): number {
    return this.droppedCount;
  }

  push(item: T): void {
    if (this.stopped) return;
    if (this.items.length >= this.max) {
      this.items.shift();
      this.droppedCount++;
    }
    this.items.push(item);
    this.waiter?.();
  }

  drain(): T[] {
    const out = this.items;
    this.items = [];
    return out;
  }

  takeDropped(): number {
    const n = this.droppedCount;
    this.droppedCount = 0;
    return n;
  }

  /** Resolves true once an item is queued, false if `timeoutMs` elapsed or the queue stopped. */
  wait(timeoutMs: number): Promise<boolean> {
    if (this.items.length > 0 || this.stopped) return Promise.resolve(this.items.length > 0);
    return new Promise<boolean>((resolve) => {
      const settle = () => {
        clearTimeout(timer);
        this.waiter = null;
        resolve(this.items.length > 0);
      };
      const timer = setTimeout(settle, timeoutMs);
      this.waiter = settle;
    });
  }

  stop(): void {
    this.stopped = true;
    this.waiter?.();
  }
}
