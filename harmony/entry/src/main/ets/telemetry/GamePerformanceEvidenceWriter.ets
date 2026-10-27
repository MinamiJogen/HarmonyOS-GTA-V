import { util } from '@kit.ArkTS';
import { fileIo } from '@kit.CoreFileKit';
import { hilog } from '@kit.PerformanceAnalysisKit';

interface EvidenceEntry { directory: string; json: string; }
let nextWriterId: number = 0;

// One async writer and at most 32 waiting records; no synchronous filesystem
// call or wait on the UI path. Overflow is reported rather than growing memory.
export class GamePerformanceEvidenceWriter {
  private pending: EvidenceEntry[] = [];
  private running: boolean = false;
  private writerId: number = ++nextWriterId;
  private sequence: number = 0;
  private dropped: number = 0;

  enqueue(directory: string, json: string): void {
    if (this.pending.length >= 32) {
      this.dropped++;
      hilog.error(0x4100, 'GamePerformance', 'Evidence queue full; dropped=%{public}d', this.dropped);
      return;
    }
    this.pending.push({ directory: directory, json: json });
    if (!this.running) { this.drain(); }
  }

  private async write(path: string, text: string, append: boolean): Promise<void> {
    const mode: number = fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE |
      (append ? fileIo.OpenMode.APPEND : fileIo.OpenMode.TRUNC);
    const file: fileIo.File = await fileIo.open(path, mode);
    try {
      const bytes: Uint8Array = new util.TextEncoder().encodeInto(text);
      const count: number = await fileIo.write(file.fd, bytes.buffer);
      if (count !== bytes.byteLength) { throw new Error('GamePerformance evidence short write'); }
    } finally { await fileIo.close(file); }
  }

  private async drain(): Promise<void> {
    this.running = true;
    try {
      while (this.pending.length > 0) {
        const entry: EvidenceEntry | undefined = this.pending.shift();
        if (entry === undefined) { continue; }
        try {
          await this.write(`${entry.directory}/game-performance-events.jsonl`, `${entry.json}\n`, true);
          const temporary: string = `${entry.directory}/game-performance-status.${this.writerId}.${++this.sequence}.tmp`;
          try {
            await this.write(temporary, entry.json, false);
            await fileIo.rename(temporary, `${entry.directory}/game-performance-status.json`);
          } catch (error) {
            try { await fileIo.unlink(temporary); } catch (_) {}
            throw new Error(String(error));
          }
        } catch (error) {
          hilog.error(0x4100, 'GamePerformance', 'Async evidence write failed: %{public}s', String(error));
        }
      }
    } finally { this.running = false; }
  }
}
