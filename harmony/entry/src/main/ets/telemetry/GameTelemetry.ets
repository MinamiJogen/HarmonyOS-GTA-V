import { taskpool } from '@kit.ArkTS';
import { gamePerformance } from '@kit.GameServiceKit';
import { hidebug } from '@kit.PerformanceAnalysisKit';

export interface CpuSample {
  appRatio: number;
  systemRatio: number;
}

export interface GpuSample {
  // Game Service Kit returns an overall device load level, not a percentage.
  loadLevel: number;
  frequencyMHz: number;
  memoryMB: number;
}

@Concurrent
function collectCpuSample(): CpuSample {
  let appRatio: number = -1;
  let systemRatio: number = -1;
  // Both synchronous APIs may perform IPC, so execute only in taskpool.
  try {
    const value: number = hidebug.getCpuUsage();
    if (Number.isFinite(value) && value >= 0) {
      appRatio = value;
    }
  } catch (_) {
    appRatio = -1;
  }
  try {
    const value: number = hidebug.getSystemCpuUsage();
    if (Number.isFinite(value) && value >= 0) {
      systemRatio = value;
    }
  } catch (_) {
    systemRatio = -1;
  }
  return { appRatio: appRatio, systemRatio: systemRatio };
}

export async function readCpuSample(): Promise<CpuSample> {
  try {
    return await taskpool.execute(collectCpuSample) as CpuSample;
  } catch (_) {
    return { appRatio: -1, systemRatio: -1 };
  }
}

async function readGpuDeviceSample(): Promise<GpuSample> {
  const sample: GpuSample = { loadLevel: -1, frequencyMHz: -1, memoryMB: -1 };
  try {
    // GPU scope must omit the optional thermal-only parameters field.
    const scope: Array<gamePerformance.DeviceInfoParameter> = [
      { deviceInfoType: gamePerformance.DeviceInfoType.GPU }
    ];
    const deviceInfo: gamePerformance.DeviceInfo = await gamePerformance.getDeviceInfoByScope(scope);
    const gpuInfo: gamePerformance.GpuInfo | undefined = deviceInfo.gpuInfo;
    if (gpuInfo !== undefined) {
      const load: number | undefined = gpuInfo.gpuLoadLevel;
      if (load !== undefined && Number.isFinite(load) && Number.isInteger(load) && load >= 1 && load <= 10) {
        sample.loadLevel = load;
      }
      const frequencyKHz: number | undefined = gpuInfo.currentFrequency;
      if (frequencyKHz !== undefined && Number.isFinite(frequencyKHz) && frequencyKHz > 0) {
        sample.frequencyMHz = frequencyKHz / 1000;
      }
    }
  } catch (_) {
    // Unsupported or unavailable readings remain unknown, not zero usage.
  }
  return sample;
}

async function readGraphicsMemoryMB(): Promise<number> {
  try {
    const memoryKB: number = await hidebug.getGraphicsMemory();
    if (Number.isFinite(memoryKB) && memoryKB >= 0) {
      return memoryKB / 1024;
    }
  } catch (_) {
    // This reading is independent of Game Service Kit GPU availability.
  }
  return -1;
}

export async function readGpuSample(): Promise<GpuSample> {
  // Start both asynchronous requests before awaiting either result.
  const gpuRequest: Promise<GpuSample> = readGpuDeviceSample();
  const memoryRequest: Promise<number> = readGraphicsMemoryMB();
  const sample: GpuSample = await gpuRequest;
  sample.memoryMB = await memoryRequest;
  return sample;
}
