import { fileIo } from '@kit.CoreFileKit';
import { util } from '@kit.ArkTS';
import { hilog } from '@kit.PerformanceAnalysisKit';

// One asynchronous write at a time. Slow storage retains only the latest
// snapshot, rather than queuing an unbounded history on the input thread.
export class LatestStatusWriter {
  private static nextId: number = 0;
  // A retired instance may already have issued an asynchronous rename. Queue
  // every instance's publication behind it so it cannot overwrite a successor.
  // File encoding/writes stay outside this short cross-instance commit queue.
  private static publication: Promise<void> = Promise.resolve();
  private instanceId: number = LatestStatusWriter.nextId++;
  private pending: string = '';
  private writing: boolean = false;
  private active: boolean = true;

  stop(): void {
    this.active = false;
    this.pending = '';
  }

  enqueue(directory: string, text: string): void {
    if (!this.active || !directory) { return; }
    this.pending = text;
    if (!this.writing) { this.drain(directory); }
  }

  private async drain(directory: string): Promise<void> {
    this.writing = true;
    const temporary: string = `${directory}/game-status.json.pending-${this.instanceId}`;
    try {
      while (this.active && this.pending) {
        const text: string = this.pending;
        this.pending = '';
        let file: fileIo.File | undefined = undefined;
        try {
          file = await fileIo.open(temporary, fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
          const bytes: Uint8Array = new util.TextEncoder().encodeInto(text);
          const written: number = await fileIo.write(file.fd, bytes.buffer);
          if (written !== bytes.byteLength) { throw new Error('incomplete status write'); }
          await fileIo.close(file);
          file = undefined;
          if (this.active) { await this.publish(temporary, `${directory}/game-status.json`); }
        } catch (error) {
          // The last complete snapshot survives an I/O failure. A subsequent
          // refresh retries with its latest state; errors never block input.
          hilog.error(0x4100, 'GameStatus', 'ASYNC_STATUS_WRITE_FAILED %{public}s', String(error));
        } finally {
          if (file !== undefined) { try { await fileIo.close(file); } catch (_) {} }
        }
      }
    } finally {
      this.writing = false;
    }
  }

  private publish(temporary: string, destination: string): Promise<void> {
    const publication: Promise<void> = LatestStatusWriter.publication.then(async () => {
      // stop() can retire this instance while it waits behind another rename.
      if (this.active) { await fileIo.rename(temporary, destination); }
    });
    // A failed commit is reported by drain, while future instances still run.
    LatestStatusWriter.publication = publication.catch(() => {});
    return publication;
  }
}
