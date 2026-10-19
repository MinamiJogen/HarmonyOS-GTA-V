import { common } from '@kit.AbilityKit';
import { fileIo } from '@kit.CoreFileKit';
import { hiAppEvent, hilog } from '@kit.PerformanceAnalysisKit';

interface ReceivedEvents {
  capturedAt: string;
  source: string;
  domain: string;
  appEventGroups: hiAppEvent.AppEventGroup[];
}

interface ExternalCopy {
  source: string;
  destination: string;
  copied: boolean;
  bytes: number;
  error: string;
}

interface CrashManifest {
  capturedAt: string;
  eventInfo: hiAppEvent.AppEventInfo;
  externalLogs: ExternalCopy[];
}

let registered: boolean = false;
let evidenceSequence: number = 0;
let pendingHolder: hiAppEvent.AppEventPackageHolder | undefined;

function writeText(path: string, text: string): void {
  const file = fileIo.openSync(path, fileIo.OpenMode.READ_WRITE | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
  try {
    fileIo.writeSync(file.fd, text);
  } finally {
    fileIo.closeSync(file);
  }
}

function newPrefix(directory: string, source: string): string {
  evidenceSequence += 1;
  return `${directory}/${source}-${Date.now()}-${evidenceSequence}`;
}

function saveEvent(directory: string, bundleName: string, info: hiAppEvent.AppEventInfo): void {
  if (info.domain !== hiAppEvent.domain.OS ||
    (info.name !== hiAppEvent.event.APP_CRASH && info.name !== hiAppEvent.event.APP_FREEZE)) {
    return;
  }
  const params = info.params as Record<string, hiAppEvent.ParamType>;
  const eventBundle: hiAppEvent.ParamType = params['bundle_name'];
  const prefix: string = newPrefix(directory, info.name);
  const copies: ExternalCopy[] = [];
  const externalLogs: hiAppEvent.ParamType = params['external_log'];
  let paths: string[] = [];
  if (typeof externalLogs === 'string') {
    paths = [externalLogs];
  } else if (Array.isArray(externalLogs)) {
    paths = externalLogs as string[];
  }
  paths.forEach((path: string, index: number) => {
    const copy: ExternalCopy = { source: path, destination: '', copied: false, bytes: 0, error: '' };
    copies.push(copy);
    try {
      if (typeof eventBundle === 'string' && eventBundle !== bundleName) {
        throw new Error('Event belongs to a different bundle.');
      }
      if (typeof path !== 'string' || !path.startsWith('/data/storage/el2/log/hiappevent/') ||
        path.split('/').includes('..')) {
        throw new Error('External log is outside this application HiAppEvent sandbox.');
      }
      const stat: fileIo.Stat = fileIo.lstatSync(path);
      if (!stat.isFile() || stat.isSymbolicLink()) {
        throw new Error('External log is not a regular application log file.');
      }
      const basename: string = path.substring(path.lastIndexOf('/') + 1).replace(/[^A-Za-z0-9._-]/g, '_');
      const destination: string = `${prefix}-external-${index}-${basename}`;
      fileIo.copyFileSync(path, destination);
      copy.destination = destination;
      copy.copied = true;
      copy.bytes = stat.size;
      hilog.info(0x4100, 'GameCrashEvidence', 'EXTERNAL_LOG_COPIED source=%{public}s output=%{public}s bytes=%{public}d',
        path, destination, stat.size);
    } catch (error) {
      copy.error = String(error);
      hilog.error(0x4100, 'GameCrashEvidence', 'EXTERNAL_LOG_COPY_FAILED source=%{public}s reason=%{public}s',
        path, copy.error);
    }
  });
  const manifest: CrashManifest = { capturedAt: new Date().toISOString(), eventInfo: info, externalLogs: copies };
  const manifestPath: string = `${prefix}.json`;
  writeText(manifestPath, JSON.stringify(manifest));
  hilog.info(0x4100, 'GameCrashEvidence', 'CRASH_EVENT_SAVED name=%{public}s output=%{public}s externalCount=%{public}d',
    info.name, manifestPath, copies.length);
}

function drainPending(directory: string, bundleName: string, holder: hiAppEvent.AppEventPackageHolder): void {
  try {
    let packages: number = 0;
    while (packages < 100) {
      const pending: hiAppEvent.AppEventPackage | null = holder.takeNext();
      if (!pending) {
        break;
      }
      packages += 1;
      const packagePath: string = `${newPrefix(directory, 'pending-package')}.json`;
      writeText(packagePath, JSON.stringify(pending));
      pending.appEventInfos.forEach((info: hiAppEvent.AppEventInfo) => { saveEvent(directory, bundleName, info); });
      hilog.info(0x4100, 'GameCrashEvidence', 'PENDING_PACKAGE_SAVED output=%{public}s rows=%{public}d', packagePath, pending.row);
    }
    hilog.info(0x4100, 'GameCrashEvidence', 'PENDING_DRAIN_COMPLETE packages=%{public}d', packages);
  } catch (error) {
    hilog.error(0x4100, 'GameCrashEvidence', 'PENDING_DRAIN_FAILED %{public}s', String(error));
  }
}

export function registerCrashEvidence(context: common.UIAbilityContext): void {
  if (registered) {
    return;
  }
  const directory: string = `${context.filesDir}/crash-evidence`;
  const bundleName: string = context.abilityInfo.bundleName;
  try {
    if (!fileIo.accessSync(directory)) {
      fileIo.mkdirSync(directory, true);
    }
    pendingHolder = hiAppEvent.addWatcher({
      name: 'HarmonyGameCrashEvidence',
      appEventFilters: [{
        domain: hiAppEvent.domain.OS,
        names: [hiAppEvent.event.APP_CRASH, hiAppEvent.event.APP_FREEZE]
      }],
      onReceive: (domain: string, groups: hiAppEvent.AppEventGroup[]) => {
        try {
          const payload: ReceivedEvents = {
            capturedAt: new Date().toISOString(), source: 'onReceive', domain: domain, appEventGroups: groups
          };
          const output: string = `${newPrefix(directory, 'onReceive')}.json`;
          writeText(output, JSON.stringify(payload));
          hilog.info(0x4100, 'GameCrashEvidence', 'ON_RECEIVE_SAVED output=%{public}s groups=%{public}d', output, groups.length);
          groups.forEach((group: hiAppEvent.AppEventGroup) => {
            group.appEventInfos.forEach((info: hiAppEvent.AppEventInfo) => { saveEvent(directory, bundleName, info); });
          });
        } catch (error) {
          hilog.error(0x4100, 'GameCrashEvidence', 'ON_RECEIVE_FAILED %{public}s', String(error));
        }
      }
    });
    if (!pendingHolder) {
      throw new Error('HiAppEvent returned no subscription holder.');
    }
    registered = true;
    writeText(`${directory}/watcher-registration.json`, JSON.stringify({
      registeredAt: new Date().toISOString(), bundleName: bundleName, directory: directory,
      holderAvailable: pendingHolder !== null && pendingHolder !== undefined
    }));
    hilog.info(0x4100, 'GameCrashEvidence', 'WATCHER_REGISTERED bundle=%{public}s output=%{public}s', bundleName, directory);
    if (pendingHolder) {
      pendingHolder.setRow(100);
      drainPending(directory, bundleName, pendingHolder);
      setTimeout(() => {
        if (pendingHolder) { drainPending(directory, bundleName, pendingHolder); }
      }, 1000);
      setTimeout(() => {
        if (pendingHolder) { drainPending(directory, bundleName, pendingHolder); }
      }, 5000);
    }
  } catch (error) {
    hilog.error(0x4100, 'GameCrashEvidence', 'WATCHER_REGISTRATION_FAILED %{public}s', String(error));
  }
}
